//! Bounded guest-suite discovery through QEMU's read-only fw_cfg interface.
//! The caller supplies register access so parsing can also run in host tests.

const FILE_DIRECTORY: u16 = 0x19;
const MAX_FILES: u32 = 1024;
const MAX_SUITE_BYTES: usize = 32;
const SUITE_FILE: &[u8] = b"opt/kutox/test-suite";
const IPV4_FILE: &[u8] = b"opt/kutox/ipv4";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestSuite {
    None,
    DriverOnly,
    DiskSnapshot,
    DiskLoad,
    StorageRead,
    StorageTests,
    Probe,
    EepromTimeout,
    PageFault,
    GeneralProtection,
    DoubleFault,
    UnexpectedPageFault,
    KernelTests,
    KernelTestsFail,
    DriverRuntimeTests,
    DriverNetworkTests,
    DriverHardeningTests,
    AsyncCoreTests,
    AsyncNetworkTests,
    NetworkTests,
    UdpTests,
    DhcpTests,
    DnsTests,
    TcpTests,
    HttpTests,
    AsyncHardeningTests,
    HeapOom,
    ThreadStackGuard,
}

impl TestSuite {
    pub const fn uses_ipv4(self) -> bool {
        matches!(
            self,
            Self::NetworkTests
                | Self::UdpTests
                | Self::DhcpTests
                | Self::DnsTests
                | Self::TcpTests
                | Self::HttpTests
        )
    }

    pub const fn selected(configured: Option<Self>) -> Self {
        match configured {
            Some(mode) => mode,
            None => Self::None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::DriverOnly => "driver-only",
            Self::DiskSnapshot => "disk-snapshot",
            Self::DiskLoad => "disk-load",
            Self::StorageRead => "storage-read",
            Self::StorageTests => "storage-tests",
            Self::Probe => "probe",
            Self::EepromTimeout => "eeprom-timeout",
            Self::PageFault => "page-fault",
            Self::GeneralProtection => "general-protection",
            Self::DoubleFault => "double-fault",
            Self::UnexpectedPageFault => "unexpected-page-fault",
            Self::KernelTests => "kernel-tests",
            Self::KernelTestsFail => "kernel-tests-fail",
            Self::DriverRuntimeTests => "driver-runtime-tests",
            Self::DriverNetworkTests => "driver-network-tests",
            Self::DriverHardeningTests => "driver-hardening-tests",
            Self::AsyncCoreTests => "async-core-tests",
            Self::AsyncNetworkTests => "async-network-tests",
            Self::NetworkTests => "network-tests",
            Self::UdpTests => "udp-tests",
            Self::DhcpTests => "dhcp-tests",
            Self::DnsTests => "dns-tests",
            Self::TcpTests => "tcp-tests",
            Self::HttpTests => "http-tests",
            Self::AsyncHardeningTests => "async-hardening-tests",
            Self::HeapOom => "heap-oom",
            Self::ThreadStackGuard => "thread-stack-guard",
        }
    }

    pub const fn is_fault_test(self) -> bool {
        matches!(
            self,
            Self::PageFault
                | Self::GeneralProtection
                | Self::DoubleFault
                | Self::UnexpectedPageFault
        )
    }

    pub fn validate_build(self) -> Result<(), Error> {
        if self.is_selected() && !cfg!(feature = "guest-tests") {
            return Err(Error::TestsDisabled);
        }
        Ok(())
    }

    pub const fn is_selected(self) -> bool {
        !matches!(self, Self::None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    DirectoryTooLarge = -20,
    MalformedDirectory = -21,
    InvalidSuiteLength = -22,
    UnknownSuite = -23,
    DuplicateSuiteFile = -24,
    TestsDisabled = -25,
    DuplicateIpv4File = -26,
    InvalidIpv4Length = -27,
    InvalidIpv4 = -28,
    DuplicateTestOptions = -29,
    InvalidTestOptions = -30,
    TestOptionsSuite = -31,
    LegacySelector = -38,
}

fn read_bytes<const N: usize>(read: &mut impl FnMut() -> u8) -> [u8; N] {
    core::array::from_fn(|_| read())
}

/// Returns `None` only when fw_cfg or the optional mode file is absent.
///
/// The caller must serialize selector/data access for the entire operation.
/// Selectors use the I/O interface's native little-endian writes, while
/// directory fields are explicitly decoded as big-endian.
pub fn read_test_suite(
    mut select: impl FnMut(u16),
    mut read: impl FnMut() -> u8,
) -> Result<Option<TestSuite>, Error> {
    if read_file::<MAX_SUITE_BYTES>(
        &mut select,
        &mut read,
        b"opt/kutox/test-mode",
        Error::DuplicateSuiteFile,
        Error::InvalidSuiteLength,
    )?
    .is_some()
    {
        return Err(Error::LegacySelector);
    }
    let Some((bytes, size)) = read_file::<MAX_SUITE_BYTES>(
        &mut select,
        &mut read,
        SUITE_FILE,
        Error::DuplicateSuiteFile,
        Error::InvalidSuiteLength,
    )?
    else {
        return Ok(None);
    };
    let value = bytes[..size].strip_suffix(&[0]).unwrap_or(&bytes[..size]);
    match value {
        b"driver-only" => Ok(Some(TestSuite::DriverOnly)),
        b"disk-snapshot" => Ok(Some(TestSuite::DiskSnapshot)),
        b"disk-load" => Ok(Some(TestSuite::DiskLoad)),
        b"storage-read" => Ok(Some(TestSuite::StorageRead)),
        b"storage-tests" => Ok(Some(TestSuite::StorageTests)),
        b"probe" => Ok(Some(TestSuite::Probe)),
        b"eeprom-timeout" => Ok(Some(TestSuite::EepromTimeout)),
        b"page-fault" => Ok(Some(TestSuite::PageFault)),
        b"general-protection" => Ok(Some(TestSuite::GeneralProtection)),
        b"double-fault" => Ok(Some(TestSuite::DoubleFault)),
        b"unexpected-page-fault" => Ok(Some(TestSuite::UnexpectedPageFault)),
        b"kernel-tests" => Ok(Some(TestSuite::KernelTests)),
        b"kernel-tests-fail" => Ok(Some(TestSuite::KernelTestsFail)),
        b"driver-runtime-tests" => Ok(Some(TestSuite::DriverRuntimeTests)),
        b"driver-network-tests" => Ok(Some(TestSuite::DriverNetworkTests)),
        b"driver-hardening-tests" => Ok(Some(TestSuite::DriverHardeningTests)),
        b"async-core-tests" => Ok(Some(TestSuite::AsyncCoreTests)),
        b"async-network-tests" => Ok(Some(TestSuite::AsyncNetworkTests)),
        b"network-tests" => Ok(Some(TestSuite::NetworkTests)),
        b"udp-tests" => Ok(Some(TestSuite::UdpTests)),
        b"dhcp-tests" => Ok(Some(TestSuite::DhcpTests)),
        b"dns-tests" => Ok(Some(TestSuite::DnsTests)),
        b"tcp-tests" => Ok(Some(TestSuite::TcpTests)),
        b"http-tests" => Ok(Some(TestSuite::HttpTests)),
        b"async-hardening-tests" => Ok(Some(TestSuite::AsyncHardeningTests)),
        b"heap-oom" => Ok(Some(TestSuite::HeapOom)),
        b"thread-stack-guard" => Ok(Some(TestSuite::ThreadStackGuard)),
        _ => Err(Error::UnknownSuite),
    }
}

pub fn read_ipv4(
    mut select: impl FnMut(u16),
    mut read: impl FnMut() -> u8,
) -> Result<crate::net::config::Mode, Error> {
    let Some((bytes, size)) = read_file::<MAX_SUITE_BYTES>(
        &mut select,
        &mut read,
        IPV4_FILE,
        Error::DuplicateIpv4File,
        Error::InvalidIpv4Length,
    )?
    else {
        return Ok(Default::default());
    };
    crate::net::config::Mode::parse(&bytes[..size]).map_err(|_| Error::InvalidIpv4)
}

pub fn read_test_options(
    mut select: impl FnMut(u16),
    mut read: impl FnMut() -> u8,
    mode: TestSuite,
) -> Result<crate::testing::selection::Selection, Error> {
    let Some((bytes, size)) = read_file::<64>(
        &mut select,
        &mut read,
        b"opt/kutox/test-options",
        Error::DuplicateTestOptions,
        Error::InvalidTestOptions,
    )?
    else {
        return Ok(Default::default());
    };
    if !cfg!(feature = "guest-tests") {
        return Err(Error::TestsDisabled);
    }
    if mode != TestSuite::KernelTests {
        return Err(Error::TestOptionsSuite);
    }
    crate::testing::selection::Selection::parse(&bytes[..size]).ok_or(Error::InvalidTestOptions)
}

fn read_file<const N: usize>(
    select: &mut impl FnMut(u16),
    read: &mut impl FnMut() -> u8,
    filename: &[u8],
    duplicate: Error,
    length: Error,
) -> Result<Option<([u8; N], usize)>, Error> {
    select(0);
    if read_bytes::<4>(read) != *b"QEMU" {
        return Ok(None);
    }
    select(FILE_DIRECTORY);
    let count = u32::from_be_bytes(read_bytes(read));
    if count > MAX_FILES {
        return Err(Error::DirectoryTooLarge);
    }
    let mut found = None;
    for _ in 0..count {
        let entry = read_bytes::<64>(read);
        let name = &entry[8..];
        let end = name
            .iter()
            .position(|byte| *byte == 0)
            .ok_or(Error::MalformedDirectory)?;
        if entry[6..8] != [0, 0] {
            return Err(Error::MalformedDirectory);
        }
        if &name[..end] != filename {
            continue;
        }
        if found.is_some() {
            return Err(duplicate);
        }
        let size = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]) as usize;
        let selector = u16::from_be_bytes([entry[4], entry[5]]);
        if selector < 0x20 || selector & 0xc000 != 0 {
            return Err(Error::MalformedDirectory);
        }
        if size == 0 || size > N {
            return Err(length);
        }
        found = Some((size, selector));
    }
    let Some((size, selector)) = found else {
        return Ok(None);
    };
    select(selector);
    let mut bytes = [0; N];
    for byte in &mut bytes[..size] {
        *byte = read();
    }
    Ok(Some((bytes, size)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap, vec::Vec};

    fn fixture(files: &[(&str, &[u8])]) -> BTreeMap<u16, Vec<u8>> {
        let mut blobs = BTreeMap::new();
        blobs.insert(0, b"QEMU".to_vec());
        let mut directory = (files.len() as u32).to_be_bytes().to_vec();
        for (index, (name, value)) in files.iter().enumerate() {
            let selector = 0x20 + index as u16;
            directory.extend_from_slice(&(value.len() as u32).to_be_bytes());
            directory.extend_from_slice(&selector.to_be_bytes());
            directory.extend_from_slice(&[0, 0]);
            let mut field = [0; 56];
            field[..name.len()].copy_from_slice(name.as_bytes());
            directory.extend_from_slice(&field);
            blobs.insert(selector, value.to_vec());
        }
        blobs.insert(FILE_DIRECTORY, directory);
        blobs
    }

    fn run(blobs: &BTreeMap<u16, Vec<u8>>) -> Result<Option<TestSuite>, Error> {
        let state = RefCell::new((0, 0));
        read_test_suite(
            |selector| *state.borrow_mut() = (selector, 0),
            || {
                let mut state = state.borrow_mut();
                let value = blobs[&state.0][state.1];
                state.1 += 1;
                value
            },
        )
    }

    #[test]
    fn absent_device_uses_default() {
        let mut blobs = BTreeMap::new();
        blobs.insert(0, b"NOPE".to_vec());
        assert_eq!(run(&blobs), Ok(None));
    }

    #[test]
    fn absent_mode_file_uses_default() {
        assert_eq!(run(&fixture(&[])), Ok(None));
        assert_eq!(run(&fixture(&[("opt/other", b"unrelated")])), Ok(None));
    }

    #[test]
    fn selects_each_supported_mode() {
        for mode in [
            TestSuite::DriverOnly,
            TestSuite::DiskSnapshot,
            TestSuite::DiskLoad,
            TestSuite::StorageRead,
            TestSuite::StorageTests,
            TestSuite::Probe,
            TestSuite::EepromTimeout,
            TestSuite::PageFault,
            TestSuite::GeneralProtection,
            TestSuite::DoubleFault,
            TestSuite::UnexpectedPageFault,
            TestSuite::KernelTests,
            TestSuite::KernelTestsFail,
            TestSuite::DriverRuntimeTests,
            TestSuite::DriverNetworkTests,
            TestSuite::DriverHardeningTests,
            TestSuite::AsyncCoreTests,
            TestSuite::AsyncNetworkTests,
            TestSuite::NetworkTests,
            TestSuite::UdpTests,
            TestSuite::DhcpTests,
            TestSuite::DnsTests,
            TestSuite::TcpTests,
            TestSuite::HttpTests,
            TestSuite::AsyncHardeningTests,
            TestSuite::HeapOom,
            TestSuite::ThreadStackGuard,
        ] {
            assert_eq!(
                run(&fixture(&[(
                    "opt/kutox/test-suite",
                    mode.name().as_bytes()
                )])),
                Ok(Some(mode))
            );
        }
    }

    #[test]
    fn allows_one_trailing_nul() {
        assert_eq!(
            run(&fixture(&[("opt/kutox/test-suite", b"kernel-tests\0")])),
            Ok(Some(TestSuite::KernelTests))
        );
    }

    #[test]
    fn scans_past_unrelated_entries() {
        assert_eq!(
            run(&fixture(&[
                ("opt/other", b"unrelated"),
                ("opt/kutox/test-suite", b"eeprom-timeout"),
                ("opt/last", b"ignored"),
            ])),
            Ok(Some(TestSuite::EepromTimeout))
        );
    }

    #[test]
    fn intermediate_proofs_require_guest_fixtures() {
        for mode in [
            TestSuite::DriverOnly,
            TestSuite::DiskSnapshot,
            TestSuite::DiskLoad,
            TestSuite::StorageRead,
            TestSuite::Probe,
        ] {
            assert_eq!(
                run(&fixture(&[(
                    "opt/kutox/test-suite",
                    mode.name().as_bytes()
                )])),
                Ok(Some(mode))
            );
            assert_eq!(
                mode.validate_build(),
                if cfg!(feature = "guest-tests") {
                    Ok(())
                } else {
                    Err(Error::TestsDisabled)
                }
            );
            assert!(mode.is_selected());
            assert!(!mode.uses_ipv4());
        }
        assert_eq!(TestSuite::selected(None), TestSuite::None);
    }

    #[test]
    fn rejects_unknown_or_embedded_nul_modes() {
        for value in [b"invalid".as_slice(), b"probe\0junk", b"probe\0\0", b"\0"] {
            assert_eq!(
                run(&fixture(&[("opt/kutox/test-suite", value)])),
                Err(Error::UnknownSuite)
            );
        }
    }

    #[test]
    fn rejects_removed_nonfatal_modes() {
        for value in [
            b"boot-memory".as_slice(),
            b"cpu-locks",
            b"breakpoint",
            b"run",
            b"network-run",
            b"user-run",
            b"file-run",
            b"disk-run",
            b"disk-demand-run",
            b"storage-run",
        ] {
            assert_eq!(
                run(&fixture(&[("opt/kutox/test-suite", value)])),
                Err(Error::UnknownSuite)
            );
        }
        assert_eq!(
            run(&fixture(&[("opt/kutox/test-mode", b"run")])),
            Err(Error::LegacySelector)
        );
    }

    #[test]
    fn bounds_directory_scanning() {
        let mut blobs = fixture(&[]);
        blobs.insert(FILE_DIRECTORY, (MAX_FILES + 1).to_be_bytes().to_vec());
        assert_eq!(run(&blobs), Err(Error::DirectoryTooLarge));
    }

    #[test]
    fn rejects_empty_or_oversized_values() {
        for size in [0u32, MAX_SUITE_BYTES as u32 + 1, u32::MAX] {
            let mut blobs = fixture(&[("opt/kutox/test-suite", b"probe")]);
            blobs.get_mut(&FILE_DIRECTORY).unwrap()[4..8].copy_from_slice(&size.to_be_bytes());
            assert_eq!(run(&blobs), Err(Error::InvalidSuiteLength));
        }
    }

    #[test]
    fn rejects_non_file_or_flagged_selectors() {
        for selector in [0x19u16, 0x4020, 0x8020] {
            let mut blobs = fixture(&[("opt/kutox/test-suite", b"probe")]);
            blobs.get_mut(&FILE_DIRECTORY).unwrap()[8..10].copy_from_slice(&selector.to_be_bytes());
            assert_eq!(run(&blobs), Err(Error::MalformedDirectory));
        }
    }

    #[test]
    fn rejects_unterminated_names() {
        let mut blobs = fixture(&[("opt/kutox/test-suite", b"probe")]);
        blobs.get_mut(&FILE_DIRECTORY).unwrap()[12..68].fill(b'x');
        assert_eq!(run(&blobs), Err(Error::MalformedDirectory));
    }

    #[test]
    fn rejects_reserved_header_bits() {
        let mut blobs = fixture(&[("opt/kutox/test-suite", b"probe")]);
        blobs.get_mut(&FILE_DIRECTORY).unwrap()[10] = 1;
        assert_eq!(run(&blobs), Err(Error::MalformedDirectory));
    }

    #[test]
    fn rejects_duplicate_mode_files() {
        assert_eq!(
            run(&fixture(&[
                ("opt/kutox/test-suite", b"probe"),
                ("opt/kutox/test-suite", b"kernel-tests"),
            ])),
            Err(Error::DuplicateSuiteFile)
        );
    }

    fn ipv4(files: &[(&str, &[u8])]) -> Result<crate::net::config::Mode, Error> {
        let blobs = fixture(files);
        let state = RefCell::new((0, 0));
        read_ipv4(
            |selector| *state.borrow_mut() = (selector, 0),
            || {
                let mut state = state.borrow_mut();
                let value = blobs[&state.0][state.1];
                state.1 += 1;
                value
            },
        )
    }

    #[test]
    fn bounded_debug_controls_validate_profile_mode_and_directory() {
        fn options(
            files: &[(&str, &[u8])],
            mode: TestSuite,
        ) -> Result<crate::testing::selection::Selection, Error> {
            let blobs = fixture(files);
            let state = RefCell::new((0, 0));
            read_test_options(
                |selector| *state.borrow_mut() = (selector, 0),
                || {
                    let mut current = state.borrow_mut();
                    let byte = blobs[&current.0][current.1];
                    current.1 += 1;
                    byte
                },
                mode,
            )
        }
        use crate::testing::selection::Selection;
        const FILE: &str = "opt/kutox/test-options";
        assert_eq!(options(&[], TestSuite::None), Ok(Selection::All));
        assert_eq!(
            options(&[(FILE, b"list")], TestSuite::KernelTests),
            if cfg!(feature = "guest-tests") {
                Ok(Selection::List)
            } else {
                Err(Error::TestsDisabled)
            }
        );
        assert_eq!(
            options(&[(FILE, b"list")], TestSuite::None),
            Err(if cfg!(feature = "guest-tests") {
                Error::TestOptionsSuite
            } else {
                Error::TestsDisabled
            })
        );
        assert_eq!(
            options(&[(FILE, b"list"), (FILE, b"list")], TestSuite::KernelTests),
            Err(Error::DuplicateTestOptions)
        );
        for value in [b"".as_slice(), &[b'x'; 65]] {
            assert_eq!(
                options(&[(FILE, value)], TestSuite::KernelTests),
                Err(Error::InvalidTestOptions)
            );
        }
        if cfg!(feature = "guest-tests") {
            for value in [b"filter=".as_slice(), b"filter=cpu.[ab]", b"list\0"] {
                assert_eq!(
                    options(&[(FILE, value)], TestSuite::KernelTests),
                    Err(Error::InvalidTestOptions)
                );
            }
            assert!(matches!(
                options(&[(FILE, b"filter=cpu.*")], TestSuite::KernelTests),
                Ok(Selection::Filter(_))
            ));
        }
    }

    #[test]
    fn bounded_optional_network_file_and_driver_independence() {
        assert_eq!(ipv4(&[]), Ok(Default::default()));
        assert_eq!(
            ipv4(&[("opt/kutox/ipv4", b"dhcp")]),
            Ok(crate::net::config::Mode::Dhcp)
        );
        assert_eq!(
            ipv4(&[("opt/kutox/ipv4", b"10.0.2.15/24")]),
            Ok(Default::default())
        );
        assert_eq!(
            ipv4(&[
                ("opt/kutox/ipv4", b"10.0.2.15/24"),
                ("opt/kutox/ipv4", b"10.0.2.16/24")
            ]),
            Err(Error::DuplicateIpv4File),
        );
        for value in [b"".as_slice(), &[b'x'; 33]] {
            assert_eq!(
                ipv4(&[("opt/kutox/ipv4", value)]),
                Err(Error::InvalidIpv4Length)
            );
        }
        assert_eq!(
            ipv4(&[("opt/kutox/ipv4", b"10.0.2.0/24")]),
            Err(Error::InvalidIpv4)
        );
        for mode in [TestSuite::DriverOnly, TestSuite::Probe] {
            assert_eq!(
                run(&fixture(&[
                    ("opt/kutox/test-suite", mode.name().as_bytes()),
                    ("opt/kutox/ipv4", b"bad"),
                    ("opt/kutox/ipv4", b"also bad"),
                ])),
                Ok(Some(mode))
            );
            assert_eq!(
                mode.validate_build(),
                if cfg!(feature = "guest-tests") {
                    Ok(())
                } else {
                    Err(Error::TestsDisabled)
                }
            );
        }
        assert_eq!(TestSuite::None.validate_build(), Ok(()));
    }

    #[test]
    fn default_network_policy_and_reserved_hardening_rejection() {
        assert!(!TestSuite::selected(None).is_selected());
        assert!(!TestSuite::None.uses_ipv4());
        assert_eq!(TestSuite::None.validate_build(), Ok(()));
        for mode in [
            TestSuite::DriverOnly,
            TestSuite::Probe,
            TestSuite::DriverRuntimeTests,
        ] {
            assert!(mode.is_selected());
            assert!(!mode.uses_ipv4());
        }
        assert!(TestSuite::NetworkTests.uses_ipv4());
        assert!(TestSuite::UdpTests.uses_ipv4());
        assert_eq!(
            run(&fixture(&[("opt/kutox/test-suite", b"udp-demo")])),
            Err(Error::UnknownSuite)
        );
        assert_eq!(
            run(&fixture(&[(
                "opt/kutox/test-suite",
                b"async-hardening-tests"
            )])),
            Ok(Some(TestSuite::AsyncHardeningTests))
        );
        assert_eq!(
            TestSuite::AsyncHardeningTests.validate_build(),
            if cfg!(feature = "guest-tests") {
                Ok(())
            } else {
                Err(Error::TestsDisabled)
            }
        );
    }
}
#[test]
fn every_selected_suite_requires_the_build_feature() {
    assert_eq!(TestSuite::selected(None), TestSuite::None);
    assert_eq!(
        TestSuite::selected(Some(TestSuite::Probe)),
        TestSuite::Probe
    );
    assert_eq!(TestSuite::None.validate_build(), Ok(()));
    assert!(!TestSuite::None.is_selected());
    for mode in [
        TestSuite::Probe,
        TestSuite::DriverOnly,
        TestSuite::DiskSnapshot,
        TestSuite::DiskLoad,
        TestSuite::StorageRead,
        TestSuite::KernelTests,
        TestSuite::KernelTestsFail,
        TestSuite::DriverRuntimeTests,
        TestSuite::DriverNetworkTests,
        TestSuite::DriverHardeningTests,
        TestSuite::AsyncCoreTests,
        TestSuite::AsyncNetworkTests,
        TestSuite::NetworkTests,
        TestSuite::UdpTests,
        TestSuite::DhcpTests,
        TestSuite::DnsTests,
        TestSuite::TcpTests,
        TestSuite::HttpTests,
        TestSuite::AsyncHardeningTests,
        TestSuite::EepromTimeout,
        TestSuite::PageFault,
        TestSuite::GeneralProtection,
        TestSuite::DoubleFault,
        TestSuite::UnexpectedPageFault,
        TestSuite::HeapOom,
        TestSuite::ThreadStackGuard,
    ] {
        assert_eq!(
            mode.validate_build(),
            if cfg!(feature = "guest-tests") {
                Ok(())
            } else {
                Err(Error::TestsDisabled)
            }
        );
    }
}
