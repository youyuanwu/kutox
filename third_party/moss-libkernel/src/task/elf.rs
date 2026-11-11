// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Eager, bounded adaptation of Moss exec_elf/process_prog_headers and
//! VMArea::{from_pheader, resolve_fault}. No VFS, VMAs or demand paging.

use super::user_stack::{self, AT_ENTRY};
use crate::memory::{
    PAGE_SIZE,
    address_space::{OwnedPage, UserAddressSpace},
    permissions::PtePermissions,
};

pub const MAX_HEADERS: usize = 16;
pub const MAX_SEGMENTS: usize = 8;
pub const MAX_LOAD_PAGES: usize = 15;
pub const AT_PHDR: u64 = 3;
pub const AT_PHENT: u64 = 4;
pub const AT_PHNUM: u64 = 5;
const HEADER_SIZE: usize = 64;
const PH_SIZE: usize = 56;
const PAGE_MASK: u64 = PAGE_SIZE as u64 - 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum LayoutError {
    Format = -350,
    Unsupported = -351,
    Capacity = -352,
    Address = -353,
    FileRange = -354,
    Permissions = -355,
    Overlap = -356,
    Entry = -357,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Error<E> {
    Memory(E),
    Stack(user_stack::LayoutError),
}

pub struct Loaded<AS> {
    pub space: AS,
    pub entry: u64,
    pub stack: u64,
    pub load_pages: usize,
}

#[derive(Clone, Copy)]
struct Segment<'a> {
    file: &'a [u8],
    start: u64,
    end: u64,
    permissions: PtePermissions,
}

fn field<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], LayoutError> {
    bytes
        .get(offset..offset.checked_add(N).ok_or(LayoutError::FileRange)?)
        .and_then(|value| value.try_into().ok())
        .ok_or(LayoutError::FileRange)
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, LayoutError> {
    Ok(u16::from_le_bytes(field(bytes, offset)?))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, LayoutError> {
    Ok(u32::from_le_bytes(field(bytes, offset)?))
}

fn u64_at(bytes: &[u8], offset: usize) -> Result<u64, LayoutError> {
    Ok(u64::from_le_bytes(field(bytes, offset)?))
}

fn file_range(bytes: &[u8], start: u64, length: u64) -> Result<&[u8], LayoutError> {
    let end = start.checked_add(length).ok_or(LayoutError::FileRange)?;
    let start = usize::try_from(start).map_err(|_| LayoutError::FileRange)?;
    let end = usize::try_from(end).map_err(|_| LayoutError::FileRange)?;
    bytes.get(start..end).ok_or(LayoutError::FileRange)
}

/// Validated static x86_64 ELF image; the last window page is reserved for stack.
pub struct Image<'a> {
    segments: [Option<Segment<'a>>; MAX_SEGMENTS],
    entry: u64,
    stack_end: u64,
    load_pages: usize,
    auxv: [(u64, u64); 4],
    aux_count: usize,
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8], user_start: u64, stack_end: u64) -> Result<Self, LayoutError> {
        let stack_start = stack_end
            .checked_sub(PAGE_SIZE as u64)
            .filter(|value| *value > user_start)
            .ok_or(LayoutError::Address)?;
        if user_start == 0
            || user_start & PAGE_MASK != 0
            || stack_end & PAGE_MASK != 0
            || stack_end > 0x0000_8000_0000_0000
        {
            return Err(LayoutError::Address);
        }
        if bytes.len() < HEADER_SIZE
            || bytes[..9] != *b"\x7fELF\x02\x01\x01\x00\x00"
            || u32_at(bytes, 20)? != 1
            || u16_at(bytes, 52)? != HEADER_SIZE as u16
        {
            return Err(LayoutError::Format);
        }
        if u16_at(bytes, 16)? != 2 || u16_at(bytes, 18)? != 62 || u32_at(bytes, 48)? != 0 {
            return Err(LayoutError::Unsupported);
        }
        let ph_offset = u64_at(bytes, 32)?;
        let ph_count = usize::from(u16_at(bytes, 56)?);
        if ph_count == 0 || ph_count > MAX_HEADERS {
            return Err(LayoutError::Capacity);
        }
        if ph_offset < HEADER_SIZE as u64 || u16_at(bytes, 54)? != PH_SIZE as u16 {
            return Err(LayoutError::Format);
        }
        let table = file_range(bytes, ph_offset, (ph_count * PH_SIZE) as u64)?;
        let entry = u64_at(bytes, 24)?;
        let mut image = Self {
            segments: [None; MAX_SEGMENTS],
            entry,
            stack_end,
            load_pages: 0,
            auxv: [
                (AT_PHNUM, ph_count as u64),
                (AT_PHENT, PH_SIZE as u64),
                (AT_ENTRY, entry),
                (AT_PHDR, 0),
            ],
            aux_count: 3,
        };
        let mut count = 0;
        let mut executable_entry = false;
        for header in table.as_chunks::<PH_SIZE>().0 {
            match u32_at(header, 0)? {
                0 | 4 | 6 => continue, // NULL, NOTE and PHDR require no additional mappings.
                0x6474_e551 => {
                    if u32_at(header, 4)? & !6 != 0 {
                        return Err(LayoutError::Permissions);
                    }
                    continue;
                }
                1 => (),
                _ => return Err(LayoutError::Unsupported),
            }
            let flags = u32_at(header, 4)?;
            let permissions =
                PtePermissions::from_raw_bits(flags & 4 != 0, flags & 2 != 0, flags & 1 != 0, true);
            if flags & !7 != 0 || !permissions.valid_user_mapping() {
                return Err(LayoutError::Permissions);
            }
            let offset = u64_at(header, 8)?;
            let address = u64_at(header, 16)?;
            let file_size = u64_at(header, 32)?;
            let memory_size = u64_at(header, 40)?;
            let align = u64_at(header, 48)?;
            if file_size > memory_size {
                return Err(LayoutError::FileRange);
            }
            file_range(bytes, offset, file_size)?;
            if address & PAGE_MASK != offset & PAGE_MASK
                || (align > 1 && (!align.is_power_of_two() || address % align != offset % align))
            {
                return Err(LayoutError::Address);
            }
            if memory_size == 0 {
                continue;
            }
            let end = address
                .checked_add(memory_size)
                .filter(|end| address >= user_start && *end <= stack_start)
                .ok_or(LayoutError::Address)?;
            let start = address & !PAGE_MASK;
            let end = (end + PAGE_MASK) & !PAGE_MASK;
            if image
                .segments
                .iter()
                .flatten()
                .any(|old| start < old.end && old.start < end)
            {
                return Err(LayoutError::Overlap);
            }
            if count == MAX_SEGMENTS {
                return Err(LayoutError::Capacity);
            }
            let pages = ((end - start) / PAGE_SIZE as u64) as usize;
            image.load_pages += pages;
            if image.load_pages > MAX_LOAD_PAGES {
                return Err(LayoutError::Capacity);
            }
            // Like Moss's mappable VMA, include the file prefix of an unaligned segment.
            let prefix = address - start;
            let file_offset = offset.checked_sub(prefix).ok_or(LayoutError::Address)?;
            let file = file_range(bytes, file_offset, file_size + prefix)?;
            if permissions.is_execute() && entry >= address && entry - address < file_size {
                executable_entry = true;
            }
            if ph_offset >= file_offset
                && ph_offset - file_offset <= file.len() as u64
                && table.len() as u64 <= file.len() as u64 - (ph_offset - file_offset)
            {
                image.auxv[3].1 = start + (ph_offset - file_offset);
                image.aux_count = 4;
            }
            image.segments[count] = Some(Segment {
                file,
                start,
                end,
                permissions,
            });
            count += 1;
        }
        if !executable_entry {
            return Err(LayoutError::Entry);
        }
        Ok(image)
    }

    /// Consumes an unpublished scratch space. Every failure drops it and all its pages.
    pub fn load<AS: UserAddressSpace>(
        &self,
        mut space: AS,
        argv: &[&str],
        envp: &[&str],
    ) -> Result<Loaded<AS>, Error<AS::Error>> {
        for segment in self.segments.iter().flatten() {
            for address in (segment.start..segment.end).step_by(PAGE_SIZE) {
                let mut page = space.allocate_zeroed_page().map_err(Error::Memory)?;
                let offset = (address - segment.start) as usize;
                // Eager equivalent of Moss resolve_fault's file/page intersection.
                if offset < segment.file.len() {
                    let length = (segment.file.len() - offset).min(PAGE_SIZE);
                    page.as_slice_mut()[..length]
                        .copy_from_slice(&segment.file[offset..offset + length]);
                }
                space
                    .map_page(page, address, segment.permissions)
                    .map_err(Error::Memory)?;
            }
        }
        let stack = user_stack::setup_user_stack(
            &mut space,
            self.stack_end,
            argv,
            envp,
            &self.auxv[..self.aux_count],
        )
        .map_err(|error| match error {
            user_stack::Error::Layout(error) => Error::Stack(error),
            user_stack::Error::Memory(error) => Error::Memory(error),
        })?;
        Ok(Loaded {
            space,
            entry: self.entry,
            stack,
            load_pages: self.load_pages,
        })
    }
}
