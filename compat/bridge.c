/* SPDX-License-Identifier: GPL-2.0-only */
#include "e1000.h"
#include "bridge.h"

extern void kutox_serial_byte(u8 byte);
extern void kutox_delay_us(u64 usec);
extern void kutox_check_boot_context(void);
extern void kutox_platform_fail(const char *operation) __attribute__((noreturn));

static uintptr_t mmio_base;
static size_t mmio_length;
static bool deny_eeprom_grant;
static u32 denied_grants;
static u32 eeprom_reads;
static bool probe_active;

_Static_assert(sizeof(struct e1000_tx_desc) == 16, "TX descriptor size");
_Static_assert(offsetof(struct e1000_tx_desc, upper) == 12, "TX status offset");

int kutox_e1000_tx_descriptor(void *ring, size_t ring_bytes,
                             u64 packet_dma, u16 packet_bytes)
{
    if (!ring || ((uintptr_t)ring & 15) || ring_bytes != 128 ||
        packet_bytes < 60 || packet_bytes > 1514 ||
        packet_dma > UINT32_MAX - packet_bytes + 1)
        return -1;
    struct e1000_tx_desc *desc = ring;
    *desc = (struct e1000_tx_desc) {
        .buffer_addr = cpu_to_le64(packet_dma),
        .lower.data = cpu_to_le32(packet_bytes | E1000_TXD_CMD_EOP |
                                 E1000_TXD_CMD_IFCS | E1000_TXD_CMD_RS),
        .upper.data = 0,
    };
    return 0;
}

static void check_address(const volatile void *address)
{
    uintptr_t value = (uintptr_t)address;
    if (!probe_active || value < mmio_base || value - mmio_base > mmio_length - 4 ||
        (value & 3))
        kutox_platform_fail("mmio_bounds");
}

u32 readl(const volatile void *address)
{
    check_address(address);
    __asm__ volatile ("" ::: "memory");
    u32 value = *(const volatile u32 *)address;
    __asm__ volatile ("lfence" ::: "memory");
    if ((uintptr_t)address == mmio_base + E1000_EECD) {
        eeprom_reads++;
        if (deny_eeprom_grant) {
            value &= ~E1000_EECD_GNT;
            denied_grants++;
        }
    }
    return value;
}

void writel(u32 value, volatile void *address)
{
    check_address(address);
    __asm__ volatile ("" ::: "memory");
    *(volatile u32 *)address = value;
    __asm__ volatile ("" ::: "memory");
}

void udelay(unsigned long usec) { kutox_delay_us(usec); }

struct net_device *e1000_get_hw_dev(struct e1000_hw *hw)
{
    struct e1000_adapter *adapter = hw->back;
    if (!adapter || !adapter->netdev)
        kutox_platform_fail("log_context");
    return adapter->netdev;
}

static void print_string(const char *string)
{
    while (*string)
        kutox_serial_byte((u8)*string++);
}

static void print_number(unsigned int value, unsigned int base)
{
    char digits[32];
    unsigned int length = 0;
    do {
        digits[length++] = "0123456789abcdef"[value % base];
        value /= base;
    } while (value);
    while (length)
        kutox_serial_byte((u8)digits[--length]);
}

void kutox_debug(const struct net_device *dev, const char *format, ...)
{
    if (!dev || !dev->name)
        kutox_platform_fail("log_context");
    print_string("LINUX_E1000 ");
    print_string(dev->name);
    print_string(": ");
    va_list arguments;
    va_start(arguments, format);
    for (; *format; format++) {
        if (*format != '%') {
            kutox_serial_byte((u8)*format);
            continue;
        }
        format++;
        switch (*format) {
        case '%': kutox_serial_byte('%'); break;
        case 's': print_string(va_arg(arguments, const char *)); break;
        case 'u': print_number(va_arg(arguments, unsigned int), 10); break;
        case 'x': print_number(va_arg(arguments, unsigned int), 16); break;
        case 'd': {
            int value = va_arg(arguments, int);
            if (value < 0)
                kutox_serial_byte('-');
            print_number(value < 0 ? 0u - (unsigned int)value : (unsigned int)value, 10);
            break;
        }
        default: kutox_platform_fail("unsupported_log_format");
        }
    }
    va_end(arguments);
}

void *phys_to_virt(unsigned long address)
{
    kutox_platform_fail("phys_to_virt");
}

void ioread16_rep(const volatile void *address, void *buffer, unsigned long count)
{
    kutox_platform_fail("ioread16_rep");
}

void iowrite16_rep(volatile void *address, const void *buffer, unsigned long count)
{
    kutox_platform_fail("iowrite16_rep");
}

void outl(u32 value, unsigned long port)
{
    kutox_platform_fail("outl");
}

void memcpy_fromio(void *destination, const volatile void *source, size_t count)
{
    kutox_platform_fail("memcpy_fromio");
}

void kutox_read_mac(uintptr_t base, size_t length, u16 vendor,
                   u16 device, u8 revision, u8 deny_grant,
                   struct kutox_result *result)
{
    *result = (struct kutox_result){ .code = -1, .stage = KUTOX_VALIDATE };
    if (probe_active || !base || (base & 4095) || length < 0x20000 ||
        base > UINTPTR_MAX - length || vendor != 0x8086 || device != E1000_DEV_ID_82540EM ||
        deny_grant > 1)
        return;
    kutox_check_boot_context();
    probe_active = true;
    mmio_base = base;
    mmio_length = length;
    deny_eeprom_grant = deny_grant;
    denied_grants = 0;
    eeprom_reads = 0;
    struct net_device netdev = { .name = "e1000-mac" };
    struct e1000_adapter adapter = { 0 };
    struct e1000_hw *hw = &adapter.hw;
    adapter.netdev = &netdev;
    hw->back = &adapter;
    hw->hw_addr = (u8 *)base;
    hw->vendor_id = vendor;
    hw->device_id = device;
    hw->revision_id = revision;
    result->stage = KUTOX_MAC_TYPE;
    result->code = e1000_set_mac_type(hw);
    if (result->code)
        goto done;
    result->stage = KUTOX_EEPROM_PARAMS;
    result->code = e1000_init_eeprom_params(hw);
    if (result->code)
        goto done;
    result->stage = KUTOX_READ_EEPROM;
    result->code = e1000_read_mac_addr(hw);
    if (result->code)
        goto done;
    for (unsigned int i = 0; i < 6; i++)
        result->mac[i] = hw->perm_mac_addr[i];
    result->stage = KUTOX_COMPLETE;
done:
    result->denied_grants = denied_grants;
    result->eeprom_reads = eeprom_reads;
    probe_active = false;
    mmio_base = 0;
    mmio_length = 0;
}
