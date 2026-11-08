use crate::{Failure, cpu::X86Cpu, serial, threads};
use kutox::{
    driver_registry::{Error, Scope},
    drivers::{Hooks, INIT_ABI, INIT_MAGIC, INIT_RECORD_BYTES, InitRecord, InitTable, Manager},
};

pub type KernelManager = Manager<X86Cpu>;

#[repr(C, align(8))]
struct InitHeader([u32; 4]);

#[used]
#[unsafe(link_section = ".kutox.native_init_header")]
static HEADER: InitHeader = InitHeader([INIT_MAGIC, INIT_ABI as u32, INIT_RECORD_BYTES as u32, 0]);

unsafe extern "C" {
    static _native_init_header: u8;
    static _native_init_header_end: u8;
    static _native_init_production_start: u8;
    static _native_init_production_end: u8;
    static _native_init_test_start: u8;
    static _native_init_test_end: u8;
    static _kernel_text_start: u8;
    static _kernel_text_end: u8;
}

pub fn linked(scope: Scope) -> Result<&'static [InitRecord], Error> {
    let header = core::ptr::addr_of!(_native_init_header) as usize;
    let header_end = core::ptr::addr_of!(_native_init_header_end) as usize;
    if !header.is_multiple_of(8) || header_end.checked_sub(header) != Some(16) {
        return Err(Error::Invalid);
    }
    // Linker-owned read-only records remain mapped for the entire kernel lifetime.
    if unsafe { (header as *const [u32; 4]).read() } != HEADER.0 {
        return Err(Error::Invalid);
    }
    let (start, end) = match scope {
        Scope::Production => (
            core::ptr::addr_of!(_native_init_production_start) as usize,
            core::ptr::addr_of!(_native_init_production_end) as usize,
        ),
        Scope::Test => (
            core::ptr::addr_of!(_native_init_test_start) as usize,
            core::ptr::addr_of!(_native_init_test_end) as usize,
        ),
    };
    let bytes = end.checked_sub(start).ok_or(Error::Invalid)?;
    if start == 0
        || !start.is_multiple_of(core::mem::align_of::<InitRecord>())
        || !bytes.is_multiple_of(INIT_RECORD_BYTES)
        || bytes / INIT_RECORD_BYTES > kutox::driver_registry::MAX_DRIVERS
    {
        return Err(Error::Invalid);
    }
    let text_start = core::ptr::addr_of!(_kernel_text_start) as usize;
    let text_end = core::ptr::addr_of!(_kernel_text_end) as usize;
    // Reject null/out-of-text addresses before reading a Rust function pointer.
    let raw = unsafe { core::slice::from_raw_parts(start as *const u8, bytes) };
    for record in raw.as_chunks::<INIT_RECORD_BYTES>().0 {
        let factory =
            usize::from_le_bytes(record[16..24].try_into().expect("fixed initializer layout"));
        if !(text_start..text_end).contains(&factory) {
            return Err(Error::Invalid);
        }
    }
    let records = unsafe {
        core::slice::from_raw_parts(start as *const InitRecord, bytes / INIT_RECORD_BYTES)
    };
    InitTable::validate(records, scope)?;
    Ok(records)
}

pub fn validate_boot_metadata() -> Result<(), Failure> {
    linked(Scope::Production)
        .map(|_| ())
        .map_err(|error| Failure::new("native_init_metadata", error.code()))
}

pub const HOOKS: Hooks = Hooks {
    progress: crate::kernel_runtime::source_progress,
    check_context: || threads::check_thread_context().map_err(|_| Error::Context),
    report_error: |operation, error| {
        serial::print(format_args!(
            "# NATIVE_DRIVER_ERROR operation={operation} code={}\n",
            error.code()
        ))
    },
};

pub fn initialize(manager: &KernelManager, scope: Scope) -> Result<(), Error> {
    let records = linked(scope)
        .inspect_err(|error| (HOOKS.report_error)("driver-linked-metadata", *error))?;
    manager.initialize_builtins(records)
}
