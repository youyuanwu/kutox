use core::arch::asm;

pub fn read8(port: u16) -> u8 {
    let value;
    unsafe { asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack)) };
    value
}

pub fn read32(port: u16) -> u32 {
    let value;
    unsafe { asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack)) };
    value
}

pub fn write8(port: u16, value: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack)) };
}

pub fn write16(port: u16, value: u16) {
    unsafe { asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack)) };
}

pub fn write32(port: u16, value: u32) {
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack)) };
}

pub fn finish(success: bool) -> ! {
    write32(0xf4, u32::from(!success));
    loop {
        unsafe { asm!("cli; hlt", options(nomem, nostack)) };
    }
}
