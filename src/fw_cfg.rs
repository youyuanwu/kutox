//! Bounded boot-mode discovery through QEMU's read-only fw_cfg interface.
//! The caller supplies register access so parsing can also run in host tests.

const FILE_DIRECTORY: u16 = 0x19;
const MAX_FILES: u32 = 1024;
const MAX_MODE_BYTES: usize = 32;
const MODE_FILE: &[u8] = b"opt/kutox/test-mode";
const IPV4_FILE: &[u8] = b"opt/kutox/ipv4";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootMode {
    Run,
    DriverRun,
    NetworkRun,
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
    HeapOom,
    ThreadStackGuard,
}

impl BootMode {
    pub const fn is_network_runtime(self) -> bool {
        matches!(self, Self::Run | Self::NetworkRun)
    }

    pub const fn uses_ipv4(self) -> bool {
        self.is_network_runtime() || matches!(self, Self::NetworkTests)
    }

    pub const fn selected(configured: Option<Self>) -> Self {
        match configured {
            Some(mode) => mode,
            None => Self::Run,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Run => "run",
            Self::DriverRun => "driver-run",
            Self::NetworkRun => "network-run",
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
        if self.is_guest_test() && !cfg!(feature = "guest-tests") {
            return Err(Error::TestsDisabled);
        }
        Ok(())
    }

    pub const fn is_guest_test(self) -> bool {
        matches!(
            self,
            Self::EepromTimeout
                | Self::PageFault
                | Self::GeneralProtection
                | Self::DoubleFault
                | Self::UnexpectedPageFault
                | Self::KernelTests
                | Self::KernelTestsFail
                | Self::DriverRuntimeTests
                | Self::DriverNetworkTests
                | Self::DriverHardeningTests
                | Self::AsyncCoreTests
                | Self::AsyncNetworkTests
                | Self::NetworkTests
                | Self::HeapOom
                | Self::ThreadStackGuard
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
pub enum Error {
    DirectoryTooLarge = -20,
    MalformedDirectory = -21,
    InvalidModeLength = -22,
    UnknownMode = -23,
    DuplicateModeFile = -24,
    TestsDisabled = -25,
    DuplicateIpv4File = -26,
    InvalidIpv4Length = -27,
    InvalidIpv4 = -28,
}

fn read_bytes<const N: usize>(read: &mut impl FnMut() -> u8) -> [u8; N] {
    core::array::from_fn(|_| read())
}

/// Returns `None` only when fw_cfg or the optional mode file is absent.
///
/// The caller must serialize selector/data access for the entire operation.
/// Selectors use the I/O interface's native little-endian writes, while
/// directory fields are explicitly decoded as big-endian.
pub fn read_mode(
    mut select: impl FnMut(u16),
    mut read: impl FnMut() -> u8,
) -> Result<Option<BootMode>, Error> {
    let Some((bytes, size)) = read_file(
        &mut select,
        &mut read,
        MODE_FILE,
        Error::DuplicateModeFile,
        Error::InvalidModeLength,
    )?
    else {
        return Ok(None);
    };
    let value = bytes[..size].strip_suffix(&[0]).unwrap_or(&bytes[..size]);
    match value {
        b"run" => Ok(Some(BootMode::Run)),
        b"driver-run" => Ok(Some(BootMode::DriverRun)),
        b"network-run" => Ok(Some(BootMode::NetworkRun)),
        b"probe" => Ok(Some(BootMode::Probe)),
        b"eeprom-timeout" => Ok(Some(BootMode::EepromTimeout)),
        b"page-fault" => Ok(Some(BootMode::PageFault)),
        b"general-protection" => Ok(Some(BootMode::GeneralProtection)),
        b"double-fault" => Ok(Some(BootMode::DoubleFault)),
        b"unexpected-page-fault" => Ok(Some(BootMode::UnexpectedPageFault)),
        b"kernel-tests" => Ok(Some(BootMode::KernelTests)),
        b"kernel-tests-fail" => Ok(Some(BootMode::KernelTestsFail)),
        b"driver-runtime-tests" => Ok(Some(BootMode::DriverRuntimeTests)),
        b"driver-network-tests" => Ok(Some(BootMode::DriverNetworkTests)),
        b"driver-hardening-tests" => Ok(Some(BootMode::DriverHardeningTests)),
        b"async-core-tests" => Ok(Some(BootMode::AsyncCoreTests)),
        b"async-network-tests" => Ok(Some(BootMode::AsyncNetworkTests)),
        b"network-tests" => Ok(Some(BootMode::NetworkTests)),
        b"async-hardening-tests" if !cfg!(feature = "guest-tests") => Err(Error::TestsDisabled),
        b"heap-oom" => Ok(Some(BootMode::HeapOom)),
        b"thread-stack-guard" => Ok(Some(BootMode::ThreadStackGuard)),
        _ => Err(Error::UnknownMode),
    }
}

pub fn read_ipv4(
    mut select: impl FnMut(u16),
    mut read: impl FnMut() -> u8,
) -> Result<crate::network_config::Config, Error> {
    let Some((bytes, size)) = read_file(
        &mut select,
        &mut read,
        IPV4_FILE,
        Error::DuplicateIpv4File,
        Error::InvalidIpv4Length,
    )?
    else {
        return Ok(Default::default());
    };
    crate::network_config::Config::parse(&bytes[..size]).map_err(|_| Error::InvalidIpv4)
}

fn read_file(
    select: &mut impl FnMut(u16),
    read: &mut impl FnMut() -> u8,
    filename: &[u8],
    duplicate: Error,
    length: Error,
) -> Result<Option<([u8; MAX_MODE_BYTES], usize)>, Error> {
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
        if size == 0 || size > MAX_MODE_BYTES {
            return Err(length);
        }
        found = Some((size, selector));
    }
    let Some((size, selector)) = found else {
        return Ok(None);
    };
    select(selector);
    let mut bytes = [0; MAX_MODE_BYTES];
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

    fn run(blobs: &BTreeMap<u16, Vec<u8>>) -> Result<Option<BootMode>, Error> {
        let state = RefCell::new((0, 0));
        read_mode(
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
            BootMode::Run,
            BootMode::DriverRun,
            BootMode::NetworkRun,
            BootMode::Probe,
            BootMode::EepromTimeout,
            BootMode::PageFault,
            BootMode::GeneralProtection,
            BootMode::DoubleFault,
            BootMode::UnexpectedPageFault,
            BootMode::KernelTests,
            BootMode::KernelTestsFail,
            BootMode::DriverRuntimeTests,
            BootMode::DriverNetworkTests,
            BootMode::DriverHardeningTests,
            BootMode::AsyncCoreTests,
            BootMode::AsyncNetworkTests,
            BootMode::NetworkTests,
            BootMode::HeapOom,
            BootMode::ThreadStackGuard,
        ] {
            assert_eq!(
                run(&fixture(&[("opt/kutox/test-mode", mode.name().as_bytes())])),
                Ok(Some(mode))
            );
        }
    }

    #[test]
    fn allows_one_trailing_nul() {
        assert_eq!(
            run(&fixture(&[("opt/kutox/test-mode", b"kernel-tests\0")])),
            Ok(Some(BootMode::KernelTests))
        );
    }

    #[test]
    fn scans_past_unrelated_entries() {
        assert_eq!(
            run(&fixture(&[
                ("opt/other", b"unrelated"),
                ("opt/kutox/test-mode", b"eeprom-timeout"),
                ("opt/last", b"ignored"),
            ])),
            Ok(Some(BootMode::EepromTimeout))
        );
    }

    #[test]
    fn rejects_unknown_or_embedded_nul_modes() {
        for value in [b"invalid".as_slice(), b"probe\0junk", b"probe\0\0", b"\0"] {
            assert_eq!(
                run(&fixture(&[("opt/kutox/test-mode", value)])),
                Err(Error::UnknownMode)
            );
        }
    }

    #[test]
    fn rejects_removed_nonfatal_modes() {
        for value in [b"boot-memory".as_slice(), b"cpu-locks", b"breakpoint"] {
            assert_eq!(
                run(&fixture(&[("opt/kutox/test-mode", value)])),
                Err(Error::UnknownMode)
            );
        }
    }

    #[test]
    fn bounds_directory_scanning() {
        let mut blobs = fixture(&[]);
        blobs.insert(FILE_DIRECTORY, (MAX_FILES + 1).to_be_bytes().to_vec());
        assert_eq!(run(&blobs), Err(Error::DirectoryTooLarge));
    }

    #[test]
    fn rejects_empty_or_oversized_values() {
        for size in [0u32, MAX_MODE_BYTES as u32 + 1, u32::MAX] {
            let mut blobs = fixture(&[("opt/kutox/test-mode", b"probe")]);
            blobs.get_mut(&FILE_DIRECTORY).unwrap()[4..8].copy_from_slice(&size.to_be_bytes());
            assert_eq!(run(&blobs), Err(Error::InvalidModeLength));
        }
    }

    #[test]
    fn rejects_non_file_or_flagged_selectors() {
        for selector in [0x19u16, 0x4020, 0x8020] {
            let mut blobs = fixture(&[("opt/kutox/test-mode", b"probe")]);
            blobs.get_mut(&FILE_DIRECTORY).unwrap()[8..10].copy_from_slice(&selector.to_be_bytes());
            assert_eq!(run(&blobs), Err(Error::MalformedDirectory));
        }
    }

    #[test]
    fn rejects_unterminated_names() {
        let mut blobs = fixture(&[("opt/kutox/test-mode", b"probe")]);
        blobs.get_mut(&FILE_DIRECTORY).unwrap()[12..68].fill(b'x');
        assert_eq!(run(&blobs), Err(Error::MalformedDirectory));
    }

    #[test]
    fn rejects_reserved_header_bits() {
        let mut blobs = fixture(&[("opt/kutox/test-mode", b"probe")]);
        blobs.get_mut(&FILE_DIRECTORY).unwrap()[10] = 1;
        assert_eq!(run(&blobs), Err(Error::MalformedDirectory));
    }

    #[test]
    fn rejects_duplicate_mode_files() {
        assert_eq!(
            run(&fixture(&[
                ("opt/kutox/test-mode", b"probe"),
                ("opt/kutox/test-mode", b"kernel-tests"),
            ])),
            Err(Error::DuplicateModeFile)
        );
    }

    fn ipv4(files: &[(&str, &[u8])]) -> Result<crate::network_config::Config, Error> {
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
    fn bounded_optional_network_file_and_driver_independence() {
        assert_eq!(ipv4(&[]), Ok(Default::default()));
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
        for mode in [BootMode::Run, BootMode::DriverRun, BootMode::Probe] {
            assert_eq!(
                run(&fixture(&[
                    ("opt/kutox/test-mode", mode.name().as_bytes()),
                    ("opt/kutox/ipv4", b"bad"),
                    ("opt/kutox/ipv4", b"also bad"),
                ])),
                Ok(Some(mode))
            );
            assert_eq!(mode.validate_build(), Ok(()));
        }
        assert_eq!(BootMode::NetworkRun.validate_build(), Ok(()));
    }

    #[test]
    fn default_network_policy_and_reserved_hardening_rejection() {
        assert!(BootMode::selected(None).is_network_runtime());
        for mode in [BootMode::Run, BootMode::NetworkRun] {
            assert!(mode.uses_ipv4());
            assert_eq!(mode.validate_build(), Ok(()));
        }
        for mode in [
            BootMode::DriverRun,
            BootMode::Probe,
            BootMode::DriverRuntimeTests,
        ] {
            assert!(!mode.is_network_runtime());
            assert!(!mode.uses_ipv4());
        }
        assert!(BootMode::NetworkTests.uses_ipv4());
        assert_eq!(
            run(&fixture(&[(
                "opt/kutox/test-mode",
                b"async-hardening-tests"
            )])),
            Err(if cfg!(feature = "guest-tests") {
                Error::UnknownMode
            } else {
                Error::TestsDisabled
            }),
        );
    }
}
#[test]
fn guest_modes_require_the_build_feature_without_changing_probe() {
    assert_eq!(BootMode::selected(None), BootMode::Run);
    assert_eq!(BootMode::selected(Some(BootMode::Probe)), BootMode::Probe);
    assert_eq!(BootMode::Run.validate_build(), Ok(()));
    assert!(!BootMode::Run.is_guest_test());
    assert_eq!(BootMode::Probe.validate_build(), Ok(()));
    for mode in [
        BootMode::KernelTests,
        BootMode::KernelTestsFail,
        BootMode::DriverRuntimeTests,
        BootMode::DriverNetworkTests,
        BootMode::DriverHardeningTests,
        BootMode::AsyncCoreTests,
        BootMode::AsyncNetworkTests,
        BootMode::NetworkTests,
        BootMode::EepromTimeout,
        BootMode::PageFault,
        BootMode::GeneralProtection,
        BootMode::DoubleFault,
        BootMode::UnexpectedPageFault,
        BootMode::HeapOom,
        BootMode::ThreadStackGuard,
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
