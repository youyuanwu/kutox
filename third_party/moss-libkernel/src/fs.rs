// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Read-only adaptation of Moss's ramdisk and ext4plus filesystem boundary.

extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::{error::Error as CoreError, fmt};
use ext4plus::prelude::{Dir, DirEntryName, Ext4, Ext4Error, Ext4Read, File, FileType};

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
            Self::Filesystem(_) => -406,
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
    let sb = &bytes[1024..2048];
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

    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, Error> {
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
        if length == 0 || length > MAX_FILE_BYTES {
            return Err(Error::FileSize);
        }
        let mut file = File::open_inode(&self.inner, inode).map_err(Error::Filesystem)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(length)
            .map_err(|_| Error::Allocation)?;
        bytes.resize(length, 0);
        let mut offset = 0;
        while offset < length {
            let end = (offset + 4096).min(length);
            let count = file
                .read_bytes(&mut bytes[offset..end])
                .map_err(Error::Filesystem)?;
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
        let filesystem = Filesystem::mount(fs::read(image).unwrap()).unwrap();
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
