// SPDX-License-Identifier: GPL-2.0-only
use super::*;
use core::{
    arch::asm,
    ptr,
    sync::atomic::{AtomicPtr, Ordering},
};
unsafe extern "C" {
    fn kutox_va_unsigned(args: *mut c_void, longs: u32) -> u64;
    fn kutox_va_signed(args: *mut c_void, longs: u32) -> i64;
    fn kutox_va_pointer(args: *mut c_void) -> *const u8;
    fn kutox_serial_byte(byte: u8);
    fn kutox_driver_tracing() -> bool;
    fn kutox_linux_ratelimit() -> bool;
}
static BEGINNING: Bsp<bool> = Bsp(UnsafeCell::new(true));
static ONCE: [AtomicPtr<c_char>; 32] = [const { AtomicPtr::new(ptr::null_mut()) }; 32];
struct Output {
    buffer: *mut u8,
    capacity: usize,
    count: usize,
    serial: bool,
}
impl Output {
    fn serial() -> Self {
        Self {
            buffer: ptr::null_mut(),
            capacity: 0,
            count: 0,
            serial: true,
        }
    }
    fn emit(&mut self, value: u8) {
        require(self.count < 4096, c"diagnostic_length");
        unsafe {
            if self.serial {
                if *BEGINNING.0.get() && kutox_driver_tracing() {
                    kutox_serial_byte(b'#');
                    kutox_serial_byte(b' ');
                }
                kutox_serial_byte(value);
                *BEGINNING.0.get() = value == b'\n';
            } else if self.capacity != 0 && self.count < self.capacity - 1 {
                self.buffer.add(self.count).write(value);
            }
        }
        self.count += 1;
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.emit(byte);
        }
    }
    unsafe fn text(&mut self, text: *const u8) {
        let len = unsafe { length(text) };
        for i in 0..len {
            self.emit(unsafe { text.add(i).read() });
        }
    }
    fn padding(&mut self, size: usize, width: usize, byte: u8) {
        for _ in size..width {
            self.emit(byte);
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn number(
        &mut self,
        mut value: u64,
        base: u64,
        width: usize,
        zero: bool,
        left: bool,
        negative: bool,
        alternate: bool,
        upper: bool,
    ) {
        let alphabet = if upper {
            b"0123456789ABCDEF"
        } else {
            b"0123456789abcdef"
        };
        require(matches!(base, 10 | 16), c"diagnostic_number_base");
        let base = if base == 16 { 16 } else { 10 };
        let mut digits = [0; 32];
        let mut used = 0;
        for digit in &mut digits {
            *digit = alphabet[(value % base) as usize];
            used += 1;
            value /= base;
            if value == 0 {
                break;
            }
        }
        require(value == 0, c"diagnostic_number_length");
        let mut size = used + usize::from(negative);
        if alternate && base == 16 {
            self.bytes(b"0x");
            size += 2;
        }
        if negative && zero {
            self.emit(b'-');
        }
        if !left {
            self.padding(size, width, if zero { b'0' } else { b' ' });
        }
        if negative && !zero {
            self.emit(b'-');
        }
        for &digit in digits.iter().take(used).rev() {
            self.emit(digit);
        }
        if left {
            self.padding(size, width, b' ');
        }
    }
}
unsafe fn length(text: *const u8) -> usize {
    require(!text.is_null(), c"diagnostic_null_string");
    for i in 0..=1024 {
        if unsafe { text.add(i).read() } == 0 {
            return i;
        }
    }
    unsafe { kutox_platform_fail(c"diagnostic_string_length".as_ptr()) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_format(
    buffer: *mut u8,
    capacity: usize,
    format: *const u8,
    args: *mut c_void,
    serial: bool,
) -> i32 {
    require(!format.is_null(), c"diagnostic_null_format");
    require(
        serial || !buffer.is_null() || capacity == 0,
        c"diagnostic_null_buffer",
    );
    let len = unsafe { length(format) };
    let mut out = Output {
        buffer,
        capacity,
        count: 0,
        serial,
    };
    let mut i = 0;
    unsafe {
        while i < len {
            let byte = format.add(i).read();
            i += 1;
            if byte != b'%' {
                out.emit(byte);
                continue;
            }
            let mut alternate = false;
            let mut left = false;
            let mut zero = false;
            if format.add(i).read() == b'#' {
                alternate = true;
                i += 1;
            }
            if format.add(i).read() == b'-' {
                left = true;
                i += 1;
            }
            if format.add(i).read() == b'0' {
                zero = true;
                i += 1;
            }
            let mut width = 0;
            while format.add(i).read().is_ascii_digit() {
                width = width * 10 + usize::from(format.add(i).read() - b'0');
                i += 1;
                require(width <= 64, c"diagnostic_width");
            }
            let mut longs = 0;
            if format.add(i).read() == b'z' {
                longs = 1;
                i += 1;
            }
            while format.add(i).read() == b'l' {
                longs += 1;
                i += 1;
                require(longs <= 2, c"diagnostic_length_modifier");
            }
            require(i < len, c"diagnostic_format");
            let kind = format.add(i).read();
            i += 1;
            require(
                longs == 0 || !matches!(kind, b's' | b'c' | b'p' | b'%'),
                c"diagnostic_modifier_type",
            );
            if kind == b'%' {
                out.emit(b'%');
                continue;
            }
            if kind == b's' {
                let text = kutox_va_pointer(args);
                let size = length(text);
                if !left {
                    out.padding(size, width, b' ');
                }
                out.text(text);
                if left {
                    out.padding(size, width, b' ');
                }
                continue;
            }
            if kind == b'c' {
                out.emit(kutox_va_signed(args, 0) as u8);
                continue;
            }
            let base = if matches!(kind, b'x' | b'X' | b'p') {
                16
            } else {
                10
            };
            let mut negative = false;
            let value = match kind {
                b'p' => {
                    let pointer = kutox_va_pointer(args);
                    if format.add(i).read() == b'M' {
                        require(!pointer.is_null(), c"diagnostic_null_mac");
                        i += 1;
                        for n in 0..6 {
                            if n != 0 {
                                out.emit(b':');
                            }
                            out.number(
                                pointer.add(n).read() as u64,
                                16,
                                2,
                                true,
                                false,
                                false,
                                false,
                                false,
                            );
                        }
                        continue;
                    }
                    width = 16;
                    zero = true;
                    pointer as usize as u64
                }
                b'd' | b'i' => {
                    let value = kutox_va_signed(args, longs);
                    negative = value < 0;
                    if negative {
                        (value as u64).wrapping_neg()
                    } else {
                        value as u64
                    }
                }
                b'u' | b'x' | b'X' => kutox_va_unsigned(args, longs),
                _ => kutox_platform_fail(c"diagnostic_format".as_ptr()),
            };
            out.number(
                value,
                base,
                width,
                zero,
                left,
                negative,
                alternate,
                kind == b'X',
            );
        }
        if !serial && capacity != 0 {
            buffer.add(out.count.min(capacity - 1)).write(0);
        }
    }
    out.count as i32
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_log_prefix(
    kind: u32,
    device: *const c_void,
    level: *const u8,
    mask: u32,
    format: *const c_char,
) -> bool {
    if kind == 4 {
        let d = device.cast::<NetDevice>();
        require(unsafe { !d.is_null() && (*d).name[0] != 0 }, c"log_context");
        let mut out = Output::serial();
        out.bytes(b"LINUX_E1000 ");
        unsafe {
            out.text((*d).name.as_ptr());
        }
        out.bytes(b": ");
        return true;
    }
    require(
        !device.is_null(),
        if kind == 1 {
            c"diagnostic_null_device"
        } else {
            c"diagnostic_null_netdev"
        },
    );
    if kind == 2 {
        require(!format.is_null(), c"warning_null");
        let mut added = false;
        for slot in &ONCE {
            let old = slot.load(Ordering::Acquire);
            if old == format.cast_mut() {
                return false;
            }
            if old.is_null() {
                match slot.compare_exchange(
                    ptr::null_mut(),
                    format.cast_mut(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                ) {
                    Ok(_) => {
                        added = true;
                        break;
                    }
                    Err(old) if old == format.cast_mut() => return false,
                    Err(_) => {}
                }
            }
        }
        require(added, c"warning_capacity");
    }
    if kind == 3 {
        let names: [&[u8]; 15] = [
            b"drv",
            b"probe",
            b"link",
            b"timer",
            b"ifdown",
            b"ifup",
            b"rx_err",
            b"tx_err",
            b"tx_queued",
            b"intr",
            b"tx_done",
            b"rx_status",
            b"pktdata",
            b"hw",
            b"wol",
        ];
        let len = unsafe { length(level) };
        let bit = names.iter().position(|name| {
            name.len() == len && (0..len).all(|i| unsafe { name[i] == level.add(i).read() })
        });
        let bit = match bit {
            Some(bit) => bit,
            None => unsafe { kutox_platform_fail(c"diagnostic_message_level".as_ptr()) },
        };
        if mask & (1 << bit) == 0 {
            return false;
        }
    }
    let mut out = Output::serial();
    out.bytes(match kind {
        1 => b"# LINUX_DEVICE ",
        2 => b"# LINUX_WARN ",
        _ => b"# LINUX_NETDEV ",
    });
    out.number(
        device as usize as u64,
        16,
        16,
        true,
        false,
        false,
        false,
        false,
    );
    out.bytes(b": ");
    true
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn warn_on(condition: bool, expression: *const u8) -> bool {
    if condition {
        let mut out = Output::serial();
        out.bytes(b"# LINUX_WARN ");
        unsafe {
            out.text(expression);
        }
        out.emit(b'\n');
    }
    condition
}
#[unsafe(no_mangle)]
pub extern "C" fn bug() -> ! {
    unsafe { kutox_platform_fail(c"linux_bug".as_ptr()) }
}
#[unsafe(no_mangle)]
pub extern "C" fn bug_on(condition: bool) {
    if condition {
        bug();
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn prefetch(pointer: *const c_void) {
    unsafe {
        asm!("prefetcht0 [{}]", in(reg) pointer, options(nostack, readonly, preserves_flags));
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn fls(value: u32) -> i32 {
    (32 - value.leading_zeros()) as i32
}
#[unsafe(no_mangle)]
pub extern "C" fn cpu_to_le16s(pointer: *mut u16) {
    require(!pointer.is_null(), c"endian_null");
}
#[unsafe(no_mangle)]
pub extern "C" fn le16_to_cpus(pointer: *mut u16) {
    require(!pointer.is_null(), c"endian_null");
}
#[unsafe(no_mangle)]
pub extern "C" fn netif_msg_init(value: i32, defaults: i32) -> u32 {
    if !(0..32).contains(&value) {
        defaults as u32
    } else {
        (1u32 << value) - 1
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn netif_msg_hw(mask: u32) -> bool {
    mask & (1 << 13) != 0
}
#[unsafe(no_mangle)]
pub extern "C" fn netif_msg_rx_status(mask: u32) -> bool {
    mask & (1 << 11) != 0
}
#[unsafe(no_mangle)]
pub extern "C" fn netif_msg_tx_done(mask: u32) -> bool {
    mask & (1 << 10) != 0
}
#[unsafe(no_mangle)]
pub extern "C" fn net_ratelimit() -> bool {
    unsafe { kutox_linux_ratelimit() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn print_hex_dump(
    level: *const c_char,
    prefix: *const u8,
    kind: i32,
    row: i32,
    group: i32,
    buffer: *const u8,
    size: usize,
    ascii: bool,
) {
    require(
        !level.is_null()
            && !prefix.is_null()
            && !buffer.is_null()
            && size <= 4096
            && matches!(row, 16 | 32)
            && group == 1
            && kind == 1
            && !ascii,
        c"hex_dump_profile",
    );
    let mut out = Output::serial();
    for i in 0..size {
        if i % row as usize == 0 {
            out.count = 0;
            unsafe {
                out.text(prefix);
            }
            out.number(i as u64, 16, 4, true, false, false, false, false);
            out.bytes(b": ");
        }
        out.number(
            unsafe { buffer.add(i).read() } as u64,
            16,
            2,
            true,
            false,
            false,
            false,
            false,
        );
        out.emit(if i % row as usize == row as usize - 1 || i == size - 1 {
            b'\n'
        } else {
            b' '
        });
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kutox_storage_log(format: *const u8, args: *mut c_void) -> i32 {
    let mut text = [0u8; 512];
    let n = unsafe { kutox_format(text.as_mut_ptr(), text.len(), format, args, false) };
    require(n >= 0 && (n as usize) < text.len(), c"storage_log_capacity");
    unsafe {
        for byte in b"# STORAGE_LINUX " {
            kutox_serial_byte(*byte);
        }
        for &byte in &text[..n as usize] {
            kutox_serial_byte(byte);
        }
        if n == 0 || text[n as usize - 1] != b'\n' {
            kutox_serial_byte(b'\n');
        }
    }
    n
}
#[unsafe(no_mangle)]
pub extern "C" fn kutox_storage_trace_callback(kind: u32, address: usize) {
    let names: [&[u8]; 5] = [b"irq", b"probe", b"queue", b"complete", b"ring"];
    require((kind as usize) < names.len(), c"storage_callback_kind");
    let mut text = [0; 128];
    let mut out = Output {
        buffer: text.as_mut_ptr(),
        capacity: text.len(),
        count: 0,
        serial: false,
    };
    out.bytes(b"# STORAGE_LINUX callback=");
    out.bytes(names[kind as usize]);
    out.bytes(b" address=");
    out.number(address as u64, 16, 0, false, false, false, false, false);
    out.emit(b'\n');
    require(out.count <= text.len(), c"storage_callback_capacity");
    unsafe {
        for &byte in &text[..out.count] {
            kutox_serial_byte(byte);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn string_get_size(
    size: u64,
    block: u64,
    _units: u32,
    buffer: *mut u8,
    capacity: i32,
) -> i32 {
    if block == 0 || size > u64::MAX / block || capacity <= 0 {
        return -22;
    }
    require(!buffer.is_null(), c"diagnostic_null_buffer");
    let mut out = Output {
        buffer,
        capacity: capacity as usize,
        count: 0,
        serial: false,
    };
    out.number(size * block, 10, 0, false, false, false, false, false);
    out.bytes(b" B");
    unsafe {
        buffer.add(out.count.min(capacity as usize - 1)).write(0);
    }
    out.count as i32
}
