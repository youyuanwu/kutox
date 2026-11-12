//! Portable bounds for the native, timer-bounded user-run boundary.

use moss_libkernel::memory::address_space::PageInfo;

pub const USER_BASE: u64 = 0x1000_0000_0000;
pub const MAX_PAGES: usize = 16;
pub const PAGE_SIZE: usize = 4096;
pub const USER_END: u64 = USER_BASE + (MAX_PAGES * PAGE_SIZE) as u64;
pub const MAX_WRITE: usize = 256;
pub const MAX_READ: usize = MAX_WRITE;
const _: () = assert!(MAX_WRITE <= PAGE_SIZE);
pub const MAX_OUTPUT: usize = 1024;
pub const MAX_SYSCALLS: usize = 32;
pub const MAX_RUN_US: u64 = 1_000_000;
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_YIELD: u64 = 24;
pub const SYS_SLEEP: u64 = 35;
pub const SYS_EXIT: u64 = 60;
pub const MAX_SLEEP_US: u64 = 1_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadRequest {
    pub address: u64,
    pub length: u64,
}

pub struct Input {
    bytes: [u8; MAX_READ],
    head: usize,
    length: usize,
    closed: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self::new()
    }
}

impl Input {
    pub const fn new() -> Self {
        Self {
            bytes: [0; MAX_READ],
            head: 0,
            length: 0,
            closed: false,
        }
    }

    pub fn ready(&self) -> bool {
        self.length != 0 || self.closed
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<usize, i64> {
        if self.closed {
            return Err(-32);
        }
        if bytes.is_empty() {
            return Ok(0);
        }
        let length = bytes.len().min(MAX_READ - self.length);
        if length == 0 {
            return Err(-11);
        }
        for &byte in &bytes[..length] {
            self.bytes[(self.head + self.length) % MAX_READ] = byte;
            self.length += 1;
        }
        Ok(length)
    }

    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn read(&mut self, bytes: &mut [u8]) -> Result<usize, i64> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if !self.ready() {
            return Err(-11);
        }
        let length = bytes.len().min(self.length);
        for byte in &mut bytes[..length] {
            *byte = self.bytes[self.head];
            self.head = (self.head + 1) % MAX_READ;
            self.length -= 1;
        }
        Ok(length)
    }
}

pub fn sleep_deadline(usec: u64, now: u64, ticks_per_us: u64) -> Result<Option<u64>, i64> {
    if usec > MAX_SLEEP_US || ticks_per_us == 0 {
        return Err(-22);
    }
    if usec == 0 {
        return Ok(None);
    }
    usec.checked_mul(ticks_per_us)
        .and_then(|ticks| now.checked_add(ticks))
        .map(Some)
        .ok_or(-22)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum BudgetError {
    Empty = -360,
    Quantum = -361,
}

pub struct ExecutionBudget {
    remaining: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SlicePlan {
    pub ticks: u64,
    pub final_slice: bool,
}

impl ExecutionBudget {
    pub fn new(ticks: u64) -> Result<Self, BudgetError> {
        if ticks == 0 {
            return Err(BudgetError::Empty);
        }
        Ok(Self { remaining: ticks })
    }

    pub fn remaining(&self) -> u64 {
        self.remaining
    }

    pub fn plan(&self, quantum_ticks: u64) -> Result<SlicePlan, BudgetError> {
        if quantum_ticks == 0 {
            return Err(BudgetError::Quantum);
        }
        if self.remaining == 0 {
            return Err(BudgetError::Empty);
        }
        Ok(SlicePlan {
            ticks: self.remaining.min(quantum_ticks),
            final_slice: self.remaining <= quantum_ticks,
        })
    }

    pub fn charge(&mut self, ticks: u64) {
        self.remaining = self.remaining.saturating_sub(ticks);
    }
}

pub fn valid_user_return(instruction: u64, stack: u64) -> bool {
    (USER_BASE..USER_END).contains(&instruction) && USER_BASE < stack && stack <= USER_END
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnAction {
    Resume,
    InvalidContext,
    TimedOut,
}

pub fn return_action(instruction: u64, stack: u64, deadline: u64, now: u64) -> ReturnAction {
    if now >= deadline {
        ReturnAction::TimedOut
    } else if !valid_user_return(instruction, stack) {
        ReturnAction::InvalidContext
    } else {
        ReturnAction::Resume
    }
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
    (flags & ALLOWED) | 0x202
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

pub fn plan_copy_to_user(
    address: u64,
    length: u64,
    mut mapping: impl FnMut(u64) -> Option<PageInfo>,
) -> Result<CopyPlan, CopyError> {
    plan_copy(address, length, |page| {
        mapping(page).filter(|info| info.permissions.is_write())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use moss_libkernel::memory::permissions::PtePermissions;

    #[test]
    fn input_short_io_wraps_and_drains_before_eof() {
        let mut input = Input::new();
        let mut bytes = [0; MAX_READ];
        assert_eq!(input.read(&mut []), Ok(0));
        assert_eq!(input.read(&mut bytes), Err(-11));
        assert!(!input.ready());
        assert_eq!(input.push(&[1; MAX_READ + 1]), Ok(MAX_READ));
        assert_eq!(input.push(&[2]), Err(-11));
        assert_eq!(input.read(&mut bytes[..MAX_READ - 2]), Ok(MAX_READ - 2));
        assert_eq!(&bytes[..MAX_READ - 2], &[1; MAX_READ - 2]);
        assert_eq!(input.push(&[2; MAX_READ]), Ok(MAX_READ - 2));
        input.close();
        input.close();
        assert_eq!(input.push(&[3]), Err(-32));
        assert_eq!(input.read(&mut bytes), Ok(MAX_READ));
        assert_eq!(&bytes[..2], &[1; 2]);
        assert_eq!(&bytes[2..], &[2; MAX_READ - 2]);
        assert!(input.ready());
        assert_eq!(input.read(&mut bytes), Ok(0));
    }

    #[test]
    fn copy_to_user_checks_writability_of_the_entire_requested_range() {
        for second in [
            None,
            Some(PtePermissions::ro(true)),
            Some(PtePermissions::rw(false)),
        ] {
            assert_eq!(
                plan_copy_to_user(USER_BASE + 4094, 4, |page| Some(PageInfo {
                    physical_address: 0x200000,
                    permissions: if page == USER_BASE {
                        PtePermissions::rw(true)
                    } else {
                        second?
                    },
                }))
                .unwrap_err(),
                CopyError::Fault
            );
        }
        assert_eq!(
            plan_copy_to_user(USER_BASE + 4094, 4, |_| Some(PageInfo {
                physical_address: 0x200000,
                permissions: PtePermissions::rw(true),
            }))
            .unwrap()
            .segments()
            .len(),
            2
        );
        assert!(plan_copy_to_user(u64::MAX, 0, |_| panic!("unexpected lookup")).is_ok());
        assert_eq!(
            plan_copy_to_user(USER_BASE, MAX_READ as u64 + 1, |_| panic!(
                "unexpected lookup"
            ))
            .unwrap_err(),
            CopyError::Length
        );
    }

    #[test]
    fn sleep_uses_one_checked_bounded_deadline_and_zero_does_not_park() {
        assert_eq!(sleep_deadline(0, u64::MAX, 2), Ok(None));
        assert_eq!(sleep_deadline(1, 10, 2), Ok(Some(12)));
        assert_eq!(sleep_deadline(MAX_SLEEP_US, 10, 2), Ok(Some(2_000_010)));
        for (usec, now, ticks) in [
            (MAX_SLEEP_US + 1, 0, 1),
            (u64::MAX, 0, 1),
            (1, 0, 0),
            (2, 0, u64::MAX),
            (1, u64::MAX, 1),
        ] {
            assert_eq!(sleep_deadline(usec, now, ticks), Err(-22));
        }
    }

    #[test]
    fn execution_budget_separates_quantum_expiry_from_exact_total_exhaustion() {
        assert!(matches!(ExecutionBudget::new(0), Err(BudgetError::Empty)));
        let mut budget = ExecutionBudget::new(10).unwrap();
        assert_eq!(budget.plan(0), Err(BudgetError::Quantum));
        assert_eq!(
            budget.plan(4),
            Ok(SlicePlan {
                ticks: 4,
                final_slice: false
            })
        );
        budget.charge(4);
        assert_eq!(budget.remaining(), 6);
        assert_eq!(
            budget.plan(6),
            Ok(SlicePlan {
                ticks: 6,
                final_slice: true
            })
        );
        assert_eq!(
            budget.plan(7),
            Ok(SlicePlan {
                ticks: 6,
                final_slice: true
            })
        );
        budget.charge(0);
        assert_eq!(budget.remaining(), 6);
        budget.charge(u64::MAX);
        assert_eq!(budget.remaining(), 0);
        assert_eq!(budget.plan(1), Err(BudgetError::Empty));
    }

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
    fn returning_flags_force_if_keep_df_and_remove_iopl_nt_vm_and_reserved_bits() {
        let sanitized = user_flags(u64::MAX);
        assert_eq!(sanitized & ((3 << 12) | (1 << 14) | (1 << 17)), 0);
        assert_ne!(sanitized & (1 << 10), 0);
        assert_eq!(sanitized >> 22, 0);
        assert_eq!(user_flags(0), 0x202);
        assert_ne!(sanitized & (1 << 9), 0);
        assert_eq!(user_flags(sanitized), sanitized);
    }

    #[test]
    fn common_return_gate_checks_exact_deadline_and_context_before_resuming() {
        assert_eq!(
            return_action(USER_BASE, USER_END, 100, 99),
            ReturnAction::Resume
        );
        for now in [100, 101, u64::MAX] {
            assert_eq!(
                return_action(USER_BASE, USER_END, 100, now),
                ReturnAction::TimedOut
            );
        }
        assert_eq!(
            return_action(USER_BASE, USER_END, u64::MAX, u64::MAX - 1),
            ReturnAction::Resume
        );
        assert_eq!(
            return_action(USER_BASE, USER_END, 0, 0),
            ReturnAction::TimedOut
        );
        for (instruction, stack) in [(0, USER_END), (USER_BASE, 0x8000_0000_0000)] {
            assert_eq!(
                return_action(instruction, stack, 100, 99),
                ReturnAction::InvalidContext
            );
        }
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
