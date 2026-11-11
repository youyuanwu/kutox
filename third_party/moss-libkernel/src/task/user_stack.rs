// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Bounded adaptation of setup_user_stack in Moss src/process/exec.rs.

use crate::memory::{
    PAGE_SIZE,
    address_space::{OwnedPage, UserAddressSpace},
    permissions::PtePermissions,
};

pub const MAX_ARGS: usize = 8;
pub const MAX_ENV: usize = 8;
pub const MAX_AUXV: usize = 8;
pub const AT_NULL: u64 = 0;
pub const AT_PAGESZ: u64 = 6;
pub const AT_ENTRY: u64 = 9;
const AT_RANDOM: u64 = 25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum LayoutError {
    StackAddress = -340,
    Capacity = -341,
    InteriorNul = -342,
    Auxv = -343,
    TooLarge = -344,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Error<E> {
    Layout(LayoutError),
    Memory(E),
}

struct StackImage {
    bytes: [u8; PAGE_SIZE],
    base: u64,
    sp: u64,
}

impl StackImage {
    fn new(
        stack_end: u64,
        argv: &[&str],
        envp: &[&str],
        auxv: &[(u64, u64)],
    ) -> Result<Self, LayoutError> {
        let base = stack_end
            .checked_sub(PAGE_SIZE as u64)
            .filter(|_| stack_end.is_multiple_of(PAGE_SIZE as u64))
            .ok_or(LayoutError::StackAddress)?;
        if argv.len() > MAX_ARGS || envp.len() > MAX_ENV || auxv.len() > MAX_AUXV {
            return Err(LayoutError::Capacity);
        }
        for (index, &(tag, _)) in auxv.iter().enumerate() {
            // Do not copy upstream's placeholder AT_RANDOM without an entropy source.
            if matches!(tag, AT_NULL | AT_PAGESZ | AT_RANDOM)
                || auxv[..index].iter().any(|&(previous, _)| previous == tag)
            {
                return Err(LayoutError::Auxv);
            }
        }
        let mut string_addrs = [0u64; MAX_ARGS + MAX_ENV];
        let count = envp.len() + argv.len();
        for (index, string) in envp.iter().chain(argv.iter()).enumerate() {
            if string.as_bytes().contains(&0) {
                return Err(LayoutError::InteriorNul);
            }
            string_addrs[index] = string.len().checked_add(1).ok_or(LayoutError::TooLarge)? as u64;
        }
        let mut current = stack_end;
        for address in string_addrs[..count].iter_mut().rev() {
            current = current
                .checked_sub(*address)
                .filter(|value| *value >= base)
                .ok_or(LayoutError::TooLarge)?;
            *address = current;
        }
        let (envp_addrs, argv_addrs) = string_addrs[..count].split_at(envp.len());
        let info_bytes = (1 + argv.len() + 1 + envp.len() + 1 + (auxv.len() + 2) * 2) * 8;
        let sp = current
            .checked_sub(info_bytes as u64)
            .map(|value| value & !15)
            .filter(|value| *value >= base)
            .ok_or(LayoutError::TooLarge)?;
        let mut image = Self {
            bytes: [0; PAGE_SIZE],
            base,
            sp,
        };
        for (index, string) in envp.iter().chain(argv.iter()).enumerate() {
            let offset = (string_addrs[index] - base) as usize;
            image.bytes[offset..offset + string.len()].copy_from_slice(string.as_bytes());
        }
        let mut offset = (sp - base) as usize;
        let mut word = |value: u64| {
            image.bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            offset += 8;
        };
        word(argv.len() as u64);
        for &address in argv_addrs {
            word(address);
        }
        word(0);
        for &address in envp_addrs {
            word(address);
        }
        word(0);
        for &(tag, value) in auxv {
            word(tag);
            word(value);
        }
        word(AT_PAGESZ);
        word(PAGE_SIZE as u64);
        word(AT_NULL);
        word(0);
        Ok(image)
    }
}

/// Maps one owned RW/NX page containing an ELF64-style initial stack.
pub fn setup_user_stack<AS: UserAddressSpace>(
    space: &mut AS,
    stack_end: u64,
    argv: &[&str],
    envp: &[&str],
    auxv: &[(u64, u64)],
) -> Result<u64, Error<AS::Error>> {
    let image = StackImage::new(stack_end, argv, envp, auxv).map_err(Error::Layout)?;
    let mut page = space.allocate_zeroed_page().map_err(Error::Memory)?;
    page.as_slice_mut().copy_from_slice(&image.bytes);
    space
        .map_page(page, image.base, PtePermissions::rw(true))
        .map_err(Error::Memory)?;
    Ok(image.sp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::address_space::PageInfo;
    use core::cell::Cell;

    const END: u64 = 0x200000;

    fn word(image: &StackImage, index: usize) -> u64 {
        let offset = (image.sp - image.base) as usize + index * 8;
        u64::from_le_bytes(image.bytes[offset..offset + 8].try_into().unwrap())
    }

    fn string(image: &StackImage, address: u64) -> &[u8] {
        let offset = (address - image.base) as usize;
        let rest = &image.bytes[offset..];
        &rest[..rest.iter().position(|byte| *byte == 0).unwrap()]
    }

    #[test]
    fn argc_argv_envp_auxv_addresses_and_alignment_match_the_serialized_bytes() {
        let image = StackImage::new(
            END,
            &["probe", "arg"],
            &["KEY=value"],
            &[(AT_ENTRY, 0x100000)],
        )
        .unwrap();
        assert_eq!(image.sp, END - 128);
        assert_eq!(image.sp % 16, 0);
        assert_eq!(word(&image, 0), 2);
        assert_eq!(string(&image, word(&image, 1)), b"probe");
        assert_eq!(string(&image, word(&image, 2)), b"arg");
        assert_eq!(word(&image, 3), 0);
        assert_eq!(string(&image, word(&image, 4)), b"KEY=value");
        assert_eq!(word(&image, 5), 0);
        assert_eq!((word(&image, 6), word(&image, 7)), (AT_ENTRY, 0x100000));
        assert_eq!((word(&image, 8), word(&image, 9)), (AT_PAGESZ, 4096));
        assert_eq!((word(&image, 10), word(&image, 11)), (AT_NULL, 0));
        assert!(
            image.bytes[..(image.sp - image.base) as usize]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn empty_inputs_and_maximum_argument_environment_and_auxv_counts_fit() {
        let empty = StackImage::new(END, &[], &[], &[]).unwrap();
        assert_eq!(empty.sp, END - 64);
        assert_eq!(
            (word(&empty, 0), word(&empty, 1), word(&empty, 2)),
            (0, 0, 0)
        );
        let aux =
            core::array::from_fn::<_, MAX_AUXV, _>(|index| (100 + index as u64, index as u64));
        assert!(StackImage::new(END, &[""; MAX_ARGS], &[""; MAX_ENV], &aux).is_ok());
        for (argv, envp, auxv) in [
            (&[""; MAX_ARGS + 1][..], &[][..], &[][..]),
            (&[][..], &[""; MAX_ENV + 1][..], &[][..]),
            (&[][..], &[][..], &[(100, 0); MAX_AUXV + 1][..]),
        ] {
            assert!(matches!(
                StackImage::new(END, argv, envp, auxv),
                Err(LayoutError::Capacity)
            ));
        }
    }

    #[test]
    fn exact_page_capacity_accepts_the_last_fitting_string_and_rejects_one_more_byte() {
        let bytes = [b'a'; PAGE_SIZE - 64];
        let fitting = core::str::from_utf8(&bytes[..bytes.len() - 1]).unwrap();
        let image = StackImage::new(END, &[fitting], &[], &[]).unwrap();
        assert_eq!(image.sp, END - PAGE_SIZE as u64);
        let oversized = core::str::from_utf8(&bytes).unwrap();
        assert!(matches!(
            StackImage::new(END, &[oversized], &[], &[]),
            Err(LayoutError::TooLarge)
        ));
    }

    #[test]
    fn invalid_stack_addresses_nuls_duplicate_reserved_and_fake_random_auxv_fail() {
        for end in [0, PAGE_SIZE as u64 - 1, END + 1] {
            assert!(matches!(
                StackImage::new(end, &[], &[], &[]),
                Err(LayoutError::StackAddress)
            ));
        }
        assert!(matches!(
            StackImage::new(END, &["a\0b"], &[], &[]),
            Err(LayoutError::InteriorNul)
        ));
        assert!(matches!(
            StackImage::new(END, &[], &["a\0b"], &[]),
            Err(LayoutError::InteriorNul)
        ));
        for auxv in [
            &[(AT_NULL, 0)][..],
            &[(AT_PAGESZ, 4096)],
            &[(AT_RANDOM, 123)],
            &[(AT_ENTRY, 1), (AT_ENTRY, 2)],
        ] {
            assert!(matches!(
                StackImage::new(END, &[], &[], auxv),
                Err(LayoutError::Auxv)
            ));
        }
    }

    struct Page<'a> {
        bytes: [u8; PAGE_SIZE],
        live: &'a Cell<usize>,
    }
    impl OwnedPage for Page<'_> {
        fn as_slice_mut(&mut self) -> &mut [u8; PAGE_SIZE] {
            &mut self.bytes
        }
    }
    impl Drop for Page<'_> {
        fn drop(&mut self) {
            self.live.set(self.live.get() - 1);
        }
    }
    struct Space<'a> {
        live: &'a Cell<usize>,
        page: Option<Page<'a>>,
        fail_alloc: bool,
        fail_map: bool,
    }
    impl<'a> UserAddressSpace for Space<'a> {
        type Error = u8;
        type Page = Page<'a>;
        fn allocate_zeroed_page(&self) -> Result<Self::Page, u8> {
            if self.fail_alloc {
                return Err(1);
            }
            self.live.set(self.live.get() + 1);
            Ok(Page {
                bytes: [0; PAGE_SIZE],
                live: self.live,
            })
        }
        fn map_page(
            &mut self,
            page: Self::Page,
            address: u64,
            permissions: PtePermissions,
        ) -> Result<(), u8> {
            assert_eq!(address, END - PAGE_SIZE as u64);
            assert_eq!(permissions, PtePermissions::rw(true));
            if self.fail_map || self.page.is_some() {
                return Err(2);
            }
            self.page = Some(page);
            Ok(())
        }
        fn unmap(&mut self, _: u64) -> Result<Self::Page, u8> {
            self.page.take().ok_or(3)
        }
        fn protect(&mut self, _: u64, _: PtePermissions) -> Result<(), u8> {
            Err(4)
        }
        fn translate(&self, _: u64) -> Result<Option<PageInfo>, u8> {
            Ok(None)
        }
    }

    #[test]
    fn mapping_transfers_the_owned_page_and_unmapping_returns_it_until_drop() {
        let live = Cell::new(0);
        let mut space = Space {
            live: &live,
            page: None,
            fail_alloc: false,
            fail_map: false,
        };
        let sp = setup_user_stack(&mut space, END, &["probe"], &[], &[]).unwrap();
        assert_eq!(live.get(), 1);
        assert_eq!(sp % 16, 0);
        let page = space.unmap(END - PAGE_SIZE as u64).unwrap();
        assert_eq!(live.get(), 1);
        drop(page);
        assert_eq!(live.get(), 0);
    }

    #[test]
    fn layout_allocation_and_mapping_failure_leave_no_page_or_existing_mapping_changed() {
        let live = Cell::new(0);
        let mut space = Space {
            live: &live,
            page: None,
            fail_alloc: false,
            fail_map: false,
        };
        assert_eq!(
            setup_user_stack(&mut space, END, &["a\0b"], &[], &[]),
            Err(Error::Layout(LayoutError::InteriorNul))
        );
        assert_eq!(live.get(), 0);
        space.fail_alloc = true;
        assert_eq!(
            setup_user_stack(&mut space, END, &[], &[], &[]),
            Err(Error::Memory(1))
        );
        space.fail_alloc = false;
        space.fail_map = true;
        assert_eq!(
            setup_user_stack(&mut space, END, &[], &[], &[]),
            Err(Error::Memory(2))
        );
        assert_eq!(live.get(), 0);
        space.fail_map = false;
        setup_user_stack(&mut space, END, &["original"], &[], &[]).unwrap();
        let before = space.page.as_ref().unwrap().bytes;
        assert_eq!(
            setup_user_stack(&mut space, END, &["replacement"], &[], &[]),
            Err(Error::Memory(2))
        );
        assert_eq!(space.page.as_ref().unwrap().bytes, before);
        assert_eq!(live.get(), 1);
        drop(space);
        assert_eq!(live.get(), 0);
    }
}
