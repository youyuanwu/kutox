use limine::{
    BaseRevision,
    request::{
        ExecutableAddressRequest, ExecutableCmdlineRequest, HhdmRequest, RequestsEndMarker,
        RequestsStartMarker, StackSizeRequest,
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
static CMDLINE: ExecutableCmdlineRequest = ExecutableCmdlineRequest::new();
#[used]
#[unsafe(link_section = ".requests")]
static STACK: StackSizeRequest = StackSizeRequest::new().with_size(64 * 1024);
#[used]
#[unsafe(link_section = ".requests_end_marker")]
static END: RequestsEndMarker = RequestsEndMarker::new();

pub struct BootInfo {
    pub hhdm_offset: u64,
    pub kernel_offset: u64,
    pub deny_eeprom_grant: bool,
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
    let deny_eeprom_grant = match cmdline {
        "" => false,
        "test=eeprom-timeout" => true,
        _ => panic!("unsupported command line: {cmdline}"),
    };
    BootInfo {
        hhdm_offset: HHDM.get_response().expect("missing HHDM").offset(),
        kernel_offset: address
            .virtual_base()
            .checked_sub(address.physical_base())
            .expect("invalid kernel address offset"),
        deny_eeprom_grant,
    }
}
