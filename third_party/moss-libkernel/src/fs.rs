// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Read-only adaptation of Moss's ramdisk and ext4plus filesystem boundary.

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::{error::Error as CoreError, fmt};
pub use ext4plus::prelude::Ext4Read;
use ext4plus::prelude::{Dir, DirEntryName, Ext4, Ext4Error, File, FileType};

pub mod disk;
pub mod files;

pub const IMAGE_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_FILE_BYTES: usize = 64 * 1024;

#[derive(Debug)]
pub enum Error {
    Image,
    Range,
    Path,
    FileSize,
    FileType,
    Allocation,
    ShortRead,
    ReadLimit,
    ReaderBusy,
    TooManyFiles,
    Filesystem(Ext4Error),
}

impl Error {
    pub fn code(&self) -> i32 {
        match self {
            Self::Image => -400,
            Self::Range => -401,
            Self::Path => -402,
            Self::FileSize => -403,
            Self::FileType => -407,
            Self::Allocation => -404,
            Self::ShortRead => -405,
            Self::ReadLimit => -408,
            Self::ReaderBusy => -409,
            Self::TooManyFiles => -410,
            Self::Filesystem(_) => -406,
        }
    }

    pub fn io_error(&self) -> Option<&(dyn CoreError + Send + Sync + 'static)> {
        match self {
            Self::Filesystem(Ext4Error::Io(error)) => Some(error.as_ref()),
            _ => None,
        }
    }

    pub fn errno(&self) -> i64 {
        match self {
            Self::Path | Self::FileSize | Self::Image => -22,
            Self::FileType => -21,
            Self::Allocation => -12,
            Self::TooManyFiles => -24,
            Self::ReadLimit => -27,
            Self::ReaderBusy => -16,
            Self::Range | Self::ShortRead => -5,
            Self::Filesystem(Ext4Error::NotFound) => -2,
            Self::Filesystem(Ext4Error::NotADirectory) => -20,
            Self::Filesystem(Ext4Error::Io(error)) => {
                error.downcast_ref::<Error>().map_or(-5, Error::errno)
            }
            Self::Filesystem(_) => -5,
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl CoreError for Error {}

pub struct Ramdisk<B> {
    bytes: B,
}

impl<B: AsRef<[u8]>> Ramdisk<B> {
    pub fn new(bytes: B) -> Self {
        Self { bytes }
    }

    pub fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<(), Error> {
        let start = usize::try_from(offset).map_err(|_| Error::Range)?;
        let end = start.checked_add(output.len()).ok_or(Error::Range)?;
        let bytes = self.bytes.as_ref().get(start..end).ok_or(Error::Range)?;
        output.copy_from_slice(bytes);
        Ok(())
    }
}

impl<B: AsRef<[u8]>> Ext4Read for Ramdisk<B> {
    fn read(&self, offset: u64, output: &mut [u8]) -> Result<(), Box<dyn CoreError + Send + Sync>> {
        self.read_at(offset, output)
            .map_err(|error| Box::new(error) as Box<dyn CoreError + Send + Sync>)
    }
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

pub fn validate_image(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() != IMAGE_BYTES {
        return Err(Error::Image);
    }
    validate_superblock(bytes[1024..2048].try_into().unwrap())
}

fn validate_superblock(sb: &[u8; 1024]) -> Result<(), Error> {
    let inodes = u32_at(sb, 0);
    // One small ext2 group bounds upstream descriptor/inode allocations.
    if sb[56..58] != [0x53, 0xef]
        || u32_at(sb, 4) != (IMAGE_BYTES / 1024) as u32
        || u32_at(sb, 20) != 1
        || u32_at(sb, 24) != 0
        || u32_at(sb, 32) != 8192
        || !(2..=128).contains(&inodes)
        || u32_at(sb, 40) != inodes
        || u32_at(sb, 76) != 1
        || sb[88..90] != [0, 1]
        || u32_at(sb, 92) != 0
        || u32_at(sb, 96) != 2
        || u32_at(sb, 100) != 0
        || sb[58..60] != [1, 0]
    {
        return Err(Error::Image);
    }
    Ok(())
}

pub struct Filesystem {
    inner: Ext4,
}

impl Filesystem {
    pub fn mount<B: AsRef<[u8]> + 'static>(bytes: B) -> Result<Self, Error> {
        validate_image(bytes.as_ref())?;
        Ext4::load(Box::new(Ramdisk::new(bytes)))
            .map(|inner| Self { inner })
            .map_err(Error::Filesystem)
    }

    pub fn mount_reader(reader: impl Ext4Read + 'static) -> Result<Self, Error> {
        let mut superblock = [0; 1024];
        reader
            .read(1024, &mut superblock)
            .map_err(|error| Error::Filesystem(Ext4Error::Io(error)))?;
        validate_superblock(&superblock)?;
        Ext4::load(Box::new(reader))
            .map(|inner| Self { inner })
            .map_err(Error::Filesystem)
    }

    pub fn lookup(&self, path: &str) -> Result<files::Inode, Error> {
        if path.len() > 128
            || !path.starts_with('/')
            || path[1..].split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || !part.is_ascii()
                    || part.contains('\0')
            })
        {
            return Err(Error::Path);
        }
        let mut inode = self.inner.read_root_inode().map_err(Error::Filesystem)?;
        for component in path[1..].split('/') {
            if inode.size_in_bytes() > IMAGE_BYTES as u64 {
                return Err(Error::FileSize);
            }
            let directory = Dir::open_inode(&self.inner, inode).map_err(Error::Filesystem)?;
            let name = DirEntryName::try_from(component.as_bytes()).map_err(|_| Error::Path)?;
            inode = directory.get_entry(name).map_err(Error::Filesystem)?;
        }
        if !matches!(inode.file_type(), FileType::Regular) {
            return Err(Error::FileType);
        }
        let length = usize::try_from(inode.size_in_bytes()).map_err(|_| Error::FileSize)?;
        if length > MAX_FILE_BYTES {
            return Err(Error::FileSize);
        }
        Ok(files::Inode::new(self.inner.clone(), inode))
    }

    pub fn open(&self, path: &str) -> Result<files::OpenFile, Error> {
        self.lookup(path).map(files::OpenFile::new)
    }

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, Error> {
        let mut file = self.open(path)?;
        let length = file.length();
        if length == 0 {
            return Err(Error::FileSize);
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| Error::Allocation)?;
        bytes.resize(length, 0);
        let mut offset = 0;
        while offset < length {
            let end = (offset + 4096).min(length);
            let count = file.read(&mut bytes[offset..end])?;
            if count == 0 {
                return Err(Error::ShortRead);
            }
            offset += count;
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;
    use std::{fs, process::Command};

    #[test]
    fn ramdisk_reads_exact_ranges_without_partial_output_on_failure() {
        let disk = Ramdisk::new(&[1, 2, 3, 4]);
        let mut bytes = [9; 2];
        disk.read_at(1, &mut bytes).unwrap();
        assert_eq!(bytes, [2, 3]);
        for offset in [3, 4, u64::MAX] {
            assert!(matches!(
                disk.read_at(offset, &mut bytes),
                Err(Error::Range)
            ));
            assert_eq!(bytes, [2, 3]);
        }
        disk.read_at(4, &mut []).unwrap();
    }

    #[test]
    fn image_admission_rejects_geometry_features_and_truncation() {
        let mut image = vec![0; IMAGE_BYTES];
        let sb = &mut image[1024..2048];
        for (offset, value) in [
            (0, 128u32),
            (4, 2048),
            (20, 1),
            (32, 8192),
            (40, 128),
            (76, 1),
            (96, 2),
        ] {
            sb[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        sb[56..58].copy_from_slice(&[0x53, 0xef]);
        sb[58] = 1;
        sb[89] = 1;
        validate_image(&image).unwrap();
        assert!(validate_image(&image[..IMAGE_BYTES - 1]).is_err());
        for offset in [
            1024, 1028, 1044, 1048, 1056, 1064, 1080, 1082, 1100, 1112, 1116, 1120, 1124,
        ] {
            image[offset] ^= 0x80;
            assert!(validate_image(&image).is_err(), "offset {offset}");
            image[offset] ^= 0x80;
        }
    }

    #[test]
    fn real_ext2_file_reads_are_bounded_and_report_lookup_errors() {
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }
        let scratch = Scratch(std::env::temp_dir().join(std::format!(
                    "kutox-ext2-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                )));
        fs::create_dir(&scratch.0).unwrap();
        let root = scratch.0.join("root");
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let content: Vec<_> = (0..8193).map(|index| (index % 251) as u8).collect();
        fs::write(root.join("bin/init"), &content).unwrap();
        fs::write(root.join("empty"), []).unwrap();
        fs::write(root.join("maximum"), vec![0xa5; MAX_FILE_BYTES]).unwrap();
        fs::write(root.join("large"), vec![0; MAX_FILE_BYTES + 1]).unwrap();
        std::os::unix::fs::symlink("bin/init", root.join("link")).unwrap();
        std::os::unix::fs::symlink("bin", root.join("linkdir")).unwrap();
        let image = scratch.0.join("root.img");
        fs::File::create(&image)
            .unwrap()
            .set_len(IMAGE_BYTES as u64)
            .unwrap();
        let result = Command::new("mke2fs")
            .args([
                "-q",
                "-F",
                "-t",
                "ext2",
                "-b",
                "1024",
                "-I",
                "256",
                "-N",
                "128",
                "-m",
                "0",
                "-O",
                "none,filetype",
                "-d",
            ])
            .arg(&root)
            .arg(&image)
            .arg("2048")
            .output()
            .expect("mke2fs is required for filesystem tests");
        assert!(
            result.status.success(),
            "{}",
            std::string::String::from_utf8_lossy(&result.stderr)
        );
        let data = fs::read(image).unwrap();
        use alloc::rc::Rc;
        use core::cell::Cell;
        struct Device {
            bytes: Vec<u8>,
            fail: Rc<Cell<bool>>,
            released: Rc<Cell<bool>>,
        }
        impl Drop for Device {
            fn drop(&mut self) {
                self.released.set(true);
            }
        }
        impl disk::ReadSectors for Device {
            type Error = Error;

            fn read_sectors(&mut self, sector: u64, output: &mut [u8]) -> Result<(), Error> {
                if self.fail.get() {
                    return Err(Error::ShortRead);
                }
                Ramdisk::new(&self.bytes).read_at(sector * 512, output)
            }
        }
        let fail = Rc::new(Cell::new(false));
        let released = Rc::new(Cell::new(false));
        let reader = disk::Reader::new(Device {
            bytes: data.clone(),
            fail: Rc::clone(&fail),
            released: Rc::clone(&released),
        });
        let statistics = reader.statistics();
        let demand = Filesystem::mount_reader(reader).unwrap();
        assert_eq!(demand.read_file("/bin/init").unwrap(), content);
        assert_eq!(
            demand.read_file("/maximum").unwrap(),
            vec![0xa5; MAX_FILE_BYTES]
        );
        assert!(matches!(demand.read_file("/large"), Err(Error::FileSize)));
        assert!(statistics.get().bytes < disk::MAX_READ_BYTES);
        let mut empty = demand.open("/empty").unwrap();
        assert_eq!(empty.read(&mut [0; 16]).unwrap(), 0);
        drop(empty);
        let inode = demand.lookup("/bin/init").unwrap();
        let mut prefix = [0; 3];
        assert_eq!(inode.read_at(1, &mut prefix).unwrap(), 3);
        assert_eq!(prefix, [1, 2, 3]);
        drop(inode);
        let mut table = files::DescriptorTable::new();
        assert_eq!(table.insert(demand.open("/bin/init").unwrap()).unwrap(), 3);
        assert_eq!(table.insert(demand.open("/bin/init").unwrap()).unwrap(), 4);
        fail.set(true);
        assert!(table.get_mut(3).unwrap().read(&mut prefix).is_err());
        fail.set(false);
        assert_eq!(table.get_mut(3).unwrap().read(&mut prefix).unwrap(), 3);
        assert_eq!(prefix, [0, 1, 2]);
        assert_eq!(table.get_mut(4).unwrap().read(&mut prefix).unwrap(), 3);
        assert_eq!(prefix, [0, 1, 2]);
        for fd in [5, 6] {
            assert_eq!(table.insert(demand.open("/bin/init").unwrap()).unwrap(), fd);
        }
        assert!(matches!(
            table.insert(demand.open("/bin/init").unwrap()),
            Err(Error::TooManyFiles)
        ));
        assert!(table.get_mut(-1).is_none() && table.get_mut(i32::MAX).is_none());
        assert!(table.remove(3).is_some());
        assert_eq!(table.insert(demand.open("/bin/init").unwrap()).unwrap(), 3);
        drop(demand);
        assert!(!released.get(), "open files retain the filesystem reader");
        drop(table);
        assert!(released.get(), "last file releases the reader");
        let filesystem = Filesystem::mount(data).unwrap();
        assert_eq!(filesystem.read_file("/bin/init").unwrap(), content);
        for path in ["/missing", "/linkdir/init"] {
            assert!(matches!(
                filesystem.read_file(path),
                Err(Error::Filesystem(_))
            ));
        }
        for path in ["/bin", "/link"] {
            assert!(matches!(filesystem.read_file(path), Err(Error::FileType)));
        }
        for path in ["/empty", "/large"] {
            assert!(matches!(filesystem.read_file(path), Err(Error::FileSize)));
        }
        for path in [
            "bin/init",
            "/",
            "/bin/../init",
            "/bin//init",
            "/bin/./init",
            "/bin/init\0",
        ] {
            assert!(matches!(filesystem.read_file(path), Err(Error::Path)));
        }
    }
}
