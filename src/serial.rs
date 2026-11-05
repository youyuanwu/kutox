use crate::io;
use core::fmt::{self, Write};

pub fn init() {
    io::write8(0x3f9, 0);
    io::write8(0x3fb, 0x80);
    io::write8(0x3f8, 1);
    io::write8(0x3f9, 0);
    io::write8(0x3fb, 3);
    io::write8(0x3fa, 0xc7);
    io::write8(0x3fc, 0x0b);
}

pub fn byte(byte: u8) {
    for _ in 0..1_000_000 {
        if io::read8(0x3fd) & 0x20 != 0 {
            io::write8(0x3f8, byte);
            return;
        }
        core::hint::spin_loop();
    }
    io::finish(false);
}

struct Serial;

impl Write for Serial {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for value in text.bytes() {
            byte(value);
        }
        Ok(())
    }
}

pub fn print(arguments: fmt::Arguments<'_>) {
    if Serial.write_fmt(arguments).is_err() {
        io::finish(false);
    }
}
