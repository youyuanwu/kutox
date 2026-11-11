//! Portable bounds for the native, cooperative user-run boundary.

use moss_libkernel::memory::address_space::PageInfo;

pub const USER_BASE: u64 = 0x1000_0000_0000;
pub const MAX_PAGES: usize = 16;
pub const PAGE_SIZE: usize = 4096;
pub const USER_END: u64 = USER_BASE + (MAX_PAGES * PAGE_SIZE) as u64;
pub const MAX_WRITE: usize = 256;
const _: () = assert!(MAX_WRITE <= PAGE_SIZE);
pub const MAX_OUTPUT: usize = 1024;
pub const MAX_SYSCALLS: usize = 32;
pub const SYS_WRITE: u64 = 1;
pub const SYS_EXIT: u64 = 60;

pub fn valid_user_return(instruction: u64, stack: u64) -> bool {
    (USER_BASE..USER_END).contains(&instruction) && USER_BASE < stack && stack <= USER_END
}

pub const fn user_flags(flags: u64) -> u64 {
    const ALLOWED: u64 = (1 << 0)
        | (1 << 2)
        | (1 << 4)
        | (1 << 6)
        | (1 << 7)
        | (1 << 8)
        | (1 << 10)
        | (1 << 11)
        | (1 << 16)
        | (1 << 18)
        | (1 << 21);
    (flags & ALLOWED) | 2
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i64)]
pub enum CopyError {
    Fault = -14,
    Length = -22,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CopySegment {
    pub physical_address: u64,
    pub length: usize,
}

#[derive(Debug)]
pub struct CopyPlan {
    segments: [CopySegment; 2],
    count: usize,
}

impl CopyPlan {
    pub fn segments(&self) -> &[CopySegment] {
        &self.segments[..self.count]
    }
}

/// Validates the whole range before any output or copy, including its second page.
pub fn plan_copy(
    address: u64,
    length: u64,
    mut mapping: impl FnMut(u64) -> Option<PageInfo>,
) -> Result<CopyPlan, CopyError> {
    if length > MAX_WRITE as u64 {
        return Err(CopyError::Length);
    }
    let mut plan = CopyPlan {
        segments: [CopySegment::default(); 2],
        count: 0,
    };
    if length == 0 {
        return Ok(plan);
    }
    let end = address.checked_add(length).ok_or(CopyError::Fault)?;
    if address < USER_BASE || end > USER_END {
        return Err(CopyError::Fault);
    }
    let mut current = address;
    while current < end {
        let page = current & !(PAGE_SIZE as u64 - 1);
        let info = mapping(page).ok_or(CopyError::Fault)?;
        if !info.permissions.is_read() || !info.permissions.is_user() {
            return Err(CopyError::Fault);
        }
        let offset = current - page;
        let length = (end - current).min(PAGE_SIZE as u64 - offset) as usize;
        let physical_address = info
            .physical_address
            .checked_add(offset)
            .filter(|physical| physical.checked_add(length as u64).is_some())
            .ok_or(CopyError::Fault)?;
        plan.segments[plan.count] = CopySegment {
            physical_address,
            length,
        };
        plan.count += 1;
        current += length as u64;
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use moss_libkernel::memory::permissions::PtePermissions;

    fn mapped(_: u64) -> Option<PageInfo> {
        Some(PageInfo {
            physical_address: 0x200000,
            permissions: PtePermissions::ro(true),
        })
    }

    #[test]
    fn return_context_rejects_noncanonical_kernel_and_outside_window_addresses() {
        assert!(valid_user_return(USER_BASE, USER_END));
        assert!(valid_user_return(USER_END - 1, USER_BASE + 1));
        for (instruction, stack) in [
            (USER_END, USER_END),
            (USER_BASE - 1, USER_END),
            (USER_BASE, USER_BASE),
            (USER_BASE, USER_END + 1),
            (USER_BASE, 0x8000_0000_0000),
            (u64::MAX, USER_END),
        ] {
            assert!(!valid_user_return(instruction, stack));
        }
    }

    #[test]
    fn returning_flags_keep_df_but_remove_if_iopl_nt_vm_and_reserved_bits() {
        let sanitized = user_flags(u64::MAX);
        assert_eq!(
            sanitized & ((1 << 9) | (3 << 12) | (1 << 14) | (1 << 17)),
            0
        );
        assert_ne!(sanitized & (1 << 10), 0);
        assert_eq!(sanitized >> 22, 0);
        assert_eq!(user_flags(0), 2);
        assert_eq!(user_flags(sanitized), sanitized);
    }

    #[test]
    fn copy_plans_boundaries_and_discontiguous_physical_pages() {
        let plan = plan_copy(USER_BASE + 4094, 4, |address| {
            Some(PageInfo {
                physical_address: if address == USER_BASE {
                    0x200000
                } else {
                    0x500000
                },
                permissions: PtePermissions::ro(true),
            })
        })
        .unwrap();
        assert_eq!(
            plan.segments(),
            [
                CopySegment {
                    physical_address: 0x200ffe,
                    length: 2
                },
                CopySegment {
                    physical_address: 0x500000,
                    length: 2
                },
            ],
        );
        assert_eq!(
            plan_copy(USER_END - 256, 256, mapped).unwrap().segments()[0].length,
            256
        );
        assert_eq!(
            plan_copy(USER_BASE, 256, mapped).unwrap().segments()[0].length,
            256
        );
    }

    #[test]
    fn holes_and_supervisor_pages_fail_before_a_plan_can_be_used() {
        let mut visited = 0;
        assert_eq!(
            plan_copy(USER_BASE + 4094, 4, |address| {
                visited += 1;
                if address == USER_BASE {
                    mapped(address)
                } else {
                    None
                }
            })
            .unwrap_err(),
            CopyError::Fault
        );
        assert_eq!(visited, 2);
        for permissions in [
            PtePermissions::ro(false),
            PtePermissions::from_raw_bits(false, false, true, true),
        ] {
            assert_eq!(
                plan_copy(USER_BASE, 1, |_| Some(PageInfo {
                    physical_address: 0x200000,
                    permissions,
                }))
                .unwrap_err(),
                CopyError::Fault,
            );
        }
    }

    #[test]
    fn overflow_and_outside_window_are_rejected_without_lookup() {
        for (address, length) in [
            (0, 1),
            (USER_BASE - 1, 2),
            (USER_END, 1),
            (USER_END - 1, 2),
            (u64::MAX, 2),
        ] {
            assert_eq!(
                plan_copy(address, length, |_| panic!("unexpected lookup")).unwrap_err(),
                CopyError::Fault,
            );
        }
        assert_eq!(
            plan_copy(USER_BASE, 257, |_| panic!("unexpected lookup")).unwrap_err(),
            CopyError::Length,
        );
    }

    #[test]
    fn zero_length_needs_no_pointer_and_unrepresentable_physical_ranges_fail() {
        assert!(
            plan_copy(u64::MAX, 0, |_| panic!("unexpected lookup"))
                .unwrap()
                .segments()
                .is_empty()
        );
        assert_eq!(
            plan_copy(USER_BASE + 1, 2, |_| Some(PageInfo {
                physical_address: u64::MAX - 1,
                permissions: PtePermissions::ro(true),
            }))
            .unwrap_err(),
            CopyError::Fault,
        );
    }
}
