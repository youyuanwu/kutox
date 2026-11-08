/* SPDX-License-Identifier: GPL-2.0-only */
#include "kutox_compat.h"
#include "diagnostics_services.h"
extern void kutox_serial_byte(u8);
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
extern bool kutox_driver_tracing(void);
static bool beginning = true;

static void emit(char value, unsigned int *count)
{
    if (*count == 4096) kutox_platform_fail("diagnostic_length");
    if (beginning && kutox_driver_tracing()) {
        kutox_serial_byte('#'); kutox_serial_byte(' ');
    }
    kutox_serial_byte((u8)value);
    beginning = value == '\n';
    (*count)++;
}
static size_t length(const char *text)
{
    if (!text) kutox_platform_fail("diagnostic_null_string");
    size_t size = 0;
    while (text[size]) {
        if (size == 1024) kutox_platform_fail("diagnostic_string_length");
        size++;
    }
    return size;
}
static void text(const char *string, unsigned int *count)
{
    size_t size = length(string);
    for (size_t i = 0; i < size; i++) emit(string[i], count);
}

int kutox_vprint(const char *format, va_list arguments)
{
    if (!format) kutox_platform_fail("diagnostic_null_format");
    (void)length(format);
    unsigned int count = 0;
    while (*format) {
        if (*format != '%') { emit(*format++, &count); continue; }
        format++;
        bool left = false, zero = false;
        if (*format == '-') { left = true; format++; }
        if (*format == '0') { zero = true; format++; }
        unsigned int width = 0;
        while (*format >= '0' && *format <= '9') {
            width = width * 10 + (unsigned int)(*format++ - '0');
            if (width > 64) kutox_platform_fail("diagnostic_width");
        }
        unsigned int longs = 0;
        while (*format == 'l') { format++; if (++longs > 2) kutox_platform_fail("diagnostic_length_modifier"); }
        char kind = *format++;
        if (longs && (kind == 's' || kind == 'c' || kind == 'p' || kind == '%'))
            kutox_platform_fail("diagnostic_modifier_type");
        if (kind == '%') { emit('%', &count); continue; }
        if (kind == 's') {
            const char *value = va_arg(arguments, const char *);
            size_t size = length(value);
            if (!left) for (size_t i = size; i < width; i++) emit(' ', &count);
            text(value, &count);
            if (left) for (size_t i = size; i < width; i++) emit(' ', &count);
            continue;
        }
        if (kind == 'c') { emit((char)va_arg(arguments, int), &count); continue; }
        u64 value;
        bool negative = false;
        unsigned int base = kind == 'x' || kind == 'X' || kind == 'p' ? 16 : 10;
        if (kind == 'p') {
            const u8 *pointer = va_arg(arguments, const void *);
            if (*format == 'M') {
                if (!pointer) kutox_platform_fail("diagnostic_null_mac");
                format++;
                for (size_t i = 0; i < 6; i++) {
                    if (i) emit(':', &count);
                    emit("0123456789abcdef"[pointer[i] >> 4], &count);
                    emit("0123456789abcdef"[pointer[i] & 15], &count);
                }
                continue;
            }
            value = (uintptr_t)pointer;
            width = 16; zero = true;
        } else if (kind == 'd' || kind == 'i') {
            s64 signed_value = longs == 2 ? va_arg(arguments, long long) :
                longs == 1 ? va_arg(arguments, long) : va_arg(arguments, int);
            negative = signed_value < 0;
            value = negative ? 0ULL - (u64)signed_value : (u64)signed_value;
        } else if (kind == 'u' || kind == 'x' || kind == 'X') {
            value = longs == 2 ? va_arg(arguments, unsigned long long) :
                longs == 1 ? va_arg(arguments, unsigned long) : va_arg(arguments, unsigned int);
        } else { kutox_platform_fail("diagnostic_format"); }
        char digits[32];
        unsigned int used = 0;
        const char *alphabet = kind == 'X' ? "0123456789ABCDEF" : "0123456789abcdef";
        do { digits[used++] = alphabet[value % base]; value /= base; } while (value);
        unsigned int size = used + negative;
        if (negative && zero) emit('-', &count);
        if (!left) for (unsigned int i = size; i < width; i++) emit(zero ? '0' : ' ', &count);
        if (negative && !zero) emit('-', &count);
        while (used) emit(digits[--used], &count);
        if (left) for (unsigned int i = size; i < width; i++) emit(' ', &count);
    }
    return (int)count;
}

int printk(const char *format, ...)
{
    va_list arguments;
    va_start(arguments, format);
    int count = kutox_vprint(format, arguments);
    va_end(arguments);
    return count;
}
void netdev_printk(const struct net_device *device, const char *format, ...)
{
    if (!device) kutox_platform_fail("diagnostic_null_netdev");
    printk("# LINUX_NETDEV %p: ", device);
    va_list arguments;
    va_start(arguments, format); kutox_vprint(format, arguments); va_end(arguments);
}
void dev_printk(const struct device *device, const char *format, ...)
{
    if (!device) kutox_platform_fail("diagnostic_null_device");
    printk("# LINUX_DEVICE %p: ", device);
    va_list arguments;
    va_start(arguments, format); kutox_vprint(format, arguments); va_end(arguments);
}
static const char *once_formats[32];
void netdev_warn_once(const struct net_device *device, const char *format, ...)
{
    if (!device || !format) kutox_platform_fail("warning_null");
    unsigned int slot;
    for (slot = 0; slot < ARRAY_SIZE(once_formats); slot++) {
        const char *old = __atomic_load_n(&once_formats[slot], __ATOMIC_ACQUIRE);
        if (old == format) return;
        if (!old) {
            if (__atomic_compare_exchange_n(&once_formats[slot], &old, format, false,
                                            __ATOMIC_ACQ_REL, __ATOMIC_ACQUIRE)) break;
            if (old == format) return;
        }
    }
    if (slot == ARRAY_SIZE(once_formats)) kutox_platform_fail("warning_capacity");
    printk("# LINUX_WARN %p: ", device);
    va_list arguments;
    va_start(arguments, format); kutox_vprint(format, arguments); va_end(arguments);
}
bool warn_on(bool condition, const char *expression)
{
    if (condition) printk("# LINUX_WARN %s\n", expression);
    return condition;
}
_Noreturn void bug(void) { kutox_platform_fail("linux_bug"); }
void bug_on(bool condition) { if (condition) bug(); }
void prefetch(const void *pointer) { __builtin_prefetch(pointer); }
int fls(unsigned int value) { return value ? 32 - __builtin_clz(value) : 0; }
void cpu_to_le16s(u16 *pointer) { if (!pointer) kutox_platform_fail("endian_null"); }
void le16_to_cpus(__le16 *pointer) { if (!pointer) kutox_platform_fail("endian_null"); }
u32 netif_msg_init(int value, int defaults) { return value < 0 || value >= 32 ? (u32)defaults : value == 0 ? 0 : (1U << value) - 1; }
bool netif_msg_hw(u32 mask) { return (mask & (1U << 13)) != 0; }
bool netif_msg_rx_status(u32 mask) { return (mask & (1U << 11)) != 0; }
bool netif_msg_tx_done(u32 mask) { return (mask & (1U << 10)) != 0; }

static bool same(const char *a, const char *b)
{
    size_t size = length(a);
    if (size != length(b)) return false;
    for (size_t i = 0; i < size; i++) if (a[i] != b[i]) return false;
    return true;
}
void netif_log(u32 mask, const char *level, const struct net_device *device, const char *format, ...)
{
    static const char *const names[] = { "drv", "probe", "link", "timer", "ifdown", "ifup",
        "rx_err", "tx_err", "tx_queued", "intr", "tx_done", "rx_status", "pktdata", "hw", "wol" };
    unsigned int bit;
    for (bit = 0; bit < ARRAY_SIZE(names); bit++) if (same(level, names[bit])) break;
    if (bit == ARRAY_SIZE(names) || !device) kutox_platform_fail("diagnostic_message_level");
    if (!(mask & (1U << bit))) return;
    printk("# LINUX_NETDEV %p: ", device);
    va_list arguments;
    va_start(arguments, format); kutox_vprint(format, arguments); va_end(arguments);
}

bool net_ratelimit(void)
{
    extern bool kutox_linux_ratelimit(void);
    return kutox_linux_ratelimit();
}
void print_hex_dump(const char *level, const char *prefix, int type, int row, int group,
                    const void *buffer, size_t size, bool ascii)
{
    if (!level || !prefix || !buffer || size > 4096 || (row != 16 && row != 32) ||
        group != 1 || type != 1 || ascii) kutox_platform_fail("hex_dump_profile");
    const u8 *bytes = buffer;
    for (size_t i = 0; i < size; i++) {
        if (i % (size_t)row == 0) printk("%s%04x: ", prefix, (unsigned int)i);
        printk("%02x%c", bytes[i], i % (size_t)row == (size_t)row - 1 || i == size - 1 ? '\n' : ' ');
    }
}
