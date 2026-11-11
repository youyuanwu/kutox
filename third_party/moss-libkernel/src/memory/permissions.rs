// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Page permissions adapted from Moss's paging/permissions.rs, without CoW.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtePermissions {
    read: bool,
    write: bool,
    execute: bool,
    user: bool,
}

impl PtePermissions {
    pub const fn from_raw_bits(read: bool, write: bool, execute: bool, user: bool) -> Self {
        Self {
            read,
            write,
            execute,
            user,
        }
    }

    pub const fn ro(user: bool) -> Self {
        Self::from_raw_bits(true, false, false, user)
    }

    pub const fn rw(user: bool) -> Self {
        Self::from_raw_bits(true, true, false, user)
    }

    pub const fn rx(user: bool) -> Self {
        Self::from_raw_bits(true, false, true, user)
    }

    pub const fn is_read(self) -> bool {
        self.read
    }

    pub const fn is_write(self) -> bool {
        self.write
    }

    pub const fn is_execute(self) -> bool {
        self.execute
    }

    pub const fn is_user(self) -> bool {
        self.user
    }

    pub const fn valid_user_mapping(self) -> bool {
        self.read && self.user && !(self.write && self.execute)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_keep_user_and_kernel_permissions_distinct() {
        for user in [false, true] {
            assert_eq!(PtePermissions::ro(user).is_user(), user);
            assert!(PtePermissions::ro(user).is_read());
            assert!(!PtePermissions::ro(user).is_write());
            assert!(!PtePermissions::ro(user).is_execute());
            assert!(PtePermissions::rw(user).is_write());
            assert!(!PtePermissions::rw(user).is_execute());
            assert!(!PtePermissions::rx(user).is_write());
            assert!(PtePermissions::rx(user).is_execute());
        }
    }

    #[test]
    fn user_policy_requires_readable_user_pages_and_rejects_writable_code() {
        for read in [false, true] {
            for write in [false, true] {
                for execute in [false, true] {
                    for user in [false, true] {
                        assert_eq!(
                            PtePermissions::from_raw_bits(read, write, execute, user)
                                .valid_user_mapping(),
                            read && user && !(write && execute),
                        );
                    }
                }
            }
        }
    }
}
