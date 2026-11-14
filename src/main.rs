#![no_std]
#![no_main]

use core::panic::PanicInfo;

#[unsafe(no_mangle)]
pub extern "C" fn kmain() -> ! {
    kutox::boot::run()
}

#[panic_handler]
fn panic(info: &PanicInfo<'_>) -> ! {
    kutox::boot::report_panic(info)
}
