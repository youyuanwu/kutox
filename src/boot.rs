use limine::{
    BaseRevision,
    request::{
        ExecutableAddressRequest, ExecutableCmdlineRequest, HhdmRequest, MemoryMapRequest,
        RequestsEndMarker, RequestsStartMarker, StackSizeRequest,
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
static CMDLINE: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static STACK: StackSizeRequest = StackSizeRequest::new().with_size(64 * 1024);
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static END: RequestsEndMarker = RequestsEndMarker::new();

pub struct BootInfo {
    pub hhdm_offset: u64,
    pub kernel_physical_start: u64,
    pub kernel_size: u64,
    pub memory_map: &'static [&'static limine::memory_map::Entry],
    pub deny_eeprom_grant: bool,
    pub test_memory: bool,
}

pub fn collect() -> BootInfo {
    assert!(REVISION.is_supported(), "unsupported Limine base revision");
    let address = ADDRESS.get_response().expect("missing executable address");
    let cmdline = CMDLINE
        .get_response()
        .expect("missing command line")
        .cmdline()
        .to_str()
        .expect("non-UTF-8 command line");
    let (deny_eeprom_grant, test_memory) = match cmdline {
        "" => (false, false),
        "test=eeprom-timeout" => (true, false),
        "test=boot-memory" => (false, true),
        _ => panic!("unsupported command line: {cmdline}"),
    };
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
    BootInfo {
        hhdm_offset: HHDM.get_response().expect("missing HHDM").offset(),
        kernel_physical_start: start
            .checked_sub(kernel_offset)
            .expect("invalid kernel start"),
        kernel_size: end.checked_sub(start).expect("invalid kernel bounds"),
        memory_map: MEMORY_MAP
            .get_response()
            .expect("missing memory map")
            .entries(),
        deny_eeprom_grant,
        test_memory,
    }
}
