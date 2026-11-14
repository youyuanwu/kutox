// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Reduced Moss inode reads, open-file cursors and descriptor ownership.

extern crate alloc;

use super::{Error, Ext4, File};
use ext4plus::inode::Inode as ExtInode;

pub const MAX_FILES: usize = 4;
pub const FIRST_FD: i32 = 3;

pub struct FileCtx {
    pos: u64,
}

pub struct Inode {
    filesystem: Ext4,
    inode: ExtInode,
}

impl Inode {
    pub(super) fn new(filesystem: Ext4, inode: ExtInode) -> Self {
        Self { filesystem, inode }
    }

    pub fn length(&self) -> usize {
        self.inode.size_in_bytes() as usize
    }

    pub fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<usize, Error> {
        let length = self.inode.size_in_bytes();
        if offset >= length || output.is_empty() {
            return Ok(0);
        }
        let count = output.len().min((length - offset) as usize);
        let mut file =
            File::open_inode(&self.filesystem, self.inode.clone()).map_err(Error::Filesystem)?;
        file.seek_to(offset).map_err(Error::Filesystem)?;
        let mut read = 0;
        while read < count {
            let bytes = file
                .read_bytes(&mut output[read..count])
                .map_err(Error::Filesystem)?;
            if bytes == 0 {
                return Err(Error::ShortRead);
            }
            read += bytes;
        }
        Ok(read)
    }
}

pub struct OpenFile {
    inode: Inode,
    context: FileCtx,
}

impl OpenFile {
    pub(super) fn new(inode: Inode) -> Self {
        Self {
            inode,
            context: FileCtx { pos: 0 },
        }
    }

    pub fn length(&self) -> usize {
        self.inode.length()
    }

    pub fn read(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        let read = self.inode.read_at(self.context.pos, output)?;
        self.context.pos += read as u64;
        Ok(read)
    }
}

pub struct DescriptorTable {
    entries: [Option<OpenFile>; MAX_FILES],
}

impl Default for DescriptorTable {
    fn default() -> Self {
        Self::new()
    }
}

impl DescriptorTable {
    pub const fn new() -> Self {
        Self {
            entries: [const { None }; MAX_FILES],
        }
    }

    pub fn insert(&mut self, file: OpenFile) -> Result<i32, Error> {
        let slot = self
            .entries
            .iter()
            .position(Option::is_none)
            .ok_or(Error::TooManyFiles)?;
        self.entries[slot] = Some(file);
        Ok(FIRST_FD + slot as i32)
    }

    pub fn get_mut(&mut self, fd: i32) -> Option<&mut OpenFile> {
        let slot = usize::try_from(fd.checked_sub(FIRST_FD)?).ok()?;
        self.entries.get_mut(slot)?.as_mut()
    }

    pub fn remove(&mut self, fd: i32) -> Option<OpenFile> {
        let slot = usize::try_from(fd.checked_sub(FIRST_FD)?).ok()?;
        self.entries.get_mut(slot)?.take()
    }

    pub fn len(&self) -> usize {
        self.entries.iter().filter(|entry| entry.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
