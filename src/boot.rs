use crate::{Failure, io};
use kutox::fw_cfg::{self, BootMode};
use limine::{
    BaseRevision,
    request::{
        ExecutableAddressRequest, ExecutableCmdlineRequest, HhdmRequest, MemoryMapRequest,
        MpRequest, RequestsEndMarker, RequestsStartMarker, RsdpRequest, StackSizeRequest,
    },
};

#[used]
#[unsafe(link_section = ".requests_start_marker")]
static START: RequestsStartMarker = RequestsStartMarker::new();
#[used]
#[unsafe(link_section = ".requests")]
static REVISION: BaseRevision = BaseRevision::new();
#[used]
#[unsafe(link_section = ".requests")]
static HHDM: HhdmRequest = HhdmRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static ADDRESS: ExecutableAddressRequest = ExecutableAddressRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static MEMORY_MAP: MemoryMapRequest = MemoryMapRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static PROCESSORS: MpRequest = MpRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static RSDP: RsdpRequest = RsdpRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static CMDLINE: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static STACK: StackSizeRequest = StackSizeRequest::new().with_size(64 * 1024);
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static END: RequestsEndMarker = RequestsEndMarker::new();

#[derive(Clone)]
pub struct BootInfo {
    pub hhdm_offset: u64,
    pub kernel_physical_start: u64,
    pub kernel_size: u64,
    pub memory_map: &'static [&'static limine::memory_map::Entry],
    pub mode: BootMode,
    pub mode_configured: bool,
    pub rsdp_physical: Option<u64>,
}

pub fn processor_count() -> Result<usize, Failure> {
    if !REVISION.is_supported() {
        return Err(Failure::new("limine_base_revision", -1));
    }
    let response = PROCESSORS
        .get_response()
        .ok_or(Failure::new("missing_processor_topology", -1))?;
    Ok(response.cpus().len())
}

pub fn collect() -> Result<BootInfo, Failure> {
    assert!(REVISION.is_supported(), "unsupported Limine base revision");
    let address = ADDRESS.get_response().expect("missing executable address");
    let cmdline = CMDLINE
        .get_response()
        .expect("missing command line")
        .cmdline()
        .to_str()
        .expect("non-UTF-8 command line");
    if !cmdline.is_empty() {
        return Err(Failure::new("boot_command_line", -1));
    }
    let configured =
        fw_cfg::read_mode(|selector| io::write16(0x510, selector), || io::read8(0x511))
            .map_err(|error| Failure::new("boot_mode", error as i32))?;
    let mode = configured.unwrap_or(BootMode::Probe);
    mode.validate_build()
        .map_err(|error| Failure::new("boot_mode", error as i32))?;
    unsafe extern "C" {
        static _kernel_start: u8;
        static _kernel_end: u8;
    }
    let start = core::ptr::addr_of!(_kernel_start) as u64;
    let end = core::ptr::addr_of!(_kernel_end) as u64;
    let kernel_offset = address
        .virtual_base()
        .checked_sub(address.physical_base())
        .expect("invalid kernel address offset");
    Ok(BootInfo {
        hhdm_offset: HHDM.get_response().expect("missing HHDM").offset(),
        kernel_physical_start: start
            .checked_sub(kernel_offset)
            .expect("invalid kernel start"),
        kernel_size: end.checked_sub(start).expect("invalid kernel bounds"),
        memory_map: MEMORY_MAP
            .get_response()
            .expect("missing memory map")
            .entries(),
        mode,
        mode_configured: configured.is_some(),
        rsdp_physical: RSDP
            .get_response()
            .map(|response| response.address() as u64),
    })
}
