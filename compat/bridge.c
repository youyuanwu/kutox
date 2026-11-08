/* SPDX-License-Identifier: GPL-2.0-only */
#include "e1000.h"
#include "bridge.h"

extern void kutox_serial_byte(u8 byte);
extern void kutox_delay_us(u64 usec);
extern void kutox_check_boot_context(void);
extern void kutox_platform_fail(const char *operation) __attribute__((noreturn));

static uintptr_t mmio_base;
static size_t mmio_length;
/* Fault injection is absent from feature-off builds; the result ABI is stable. */
#if KUTOX_GUEST_TESTS
static bool deny_eeprom_grant;
static u32 denied_grants;
#endif
static u32 eeprom_reads;
static bool probe_active;
extern bool kutox_driver_mmio(uintptr_t, u32, bool);
extern int kutox_vprint(const char *, va_list);
#ifdef KUTOX_E1000_FULL_SOURCE
extern void kutox_c_tx_mmio(uintptr_t, u32, bool);
extern void kutox_c_rx_mmio(uintptr_t, u32, bool);
#endif

static void check_address(const volatile void *address, u32 value, bool writing)
{
    uintptr_t pointer = (uintptr_t)address;
    if (!probe_active && kutox_driver_mmio(pointer, value, writing)) return;
    if (!probe_active || pointer < mmio_base || pointer - mmio_base > mmio_length - 4 ||
        (pointer & 3))
        kutox_platform_fail("mmio_bounds");
}

u32 readl(const volatile void *address)
{
    check_address(address, 0, false);
    __asm__ volatile ("" ::: "memory");
    u32 value = *(const volatile u32 *)address;
    __asm__ volatile ("lfence" ::: "memory");
#ifdef KUTOX_E1000_FULL_SOURCE
    if (!probe_active) {
        kutox_c_tx_mmio((uintptr_t)address,value,false);
        kutox_c_rx_mmio((uintptr_t)address,value,false);
    }
#endif
    if (probe_active && (uintptr_t)address == mmio_base + E1000_EECD) {
        eeprom_reads++;
#if KUTOX_GUEST_TESTS
        if (deny_eeprom_grant) {
            value &= ~E1000_EECD_GNT;
            denied_grants++;
        }
#endif
    }
    return value;
}

void writel(u32 value, volatile void *address)
{
    check_address(address, value, true);
    __asm__ volatile ("" ::: "memory");
    *(volatile u32 *)address = value;
    __asm__ volatile ("" ::: "memory");
#ifdef KUTOX_E1000_FULL_SOURCE
    if (!probe_active) {
        kutox_c_tx_mmio((uintptr_t)address,value,true);
        kutox_c_rx_mmio((uintptr_t)address,value,true);
    }
#endif
}

void udelay(unsigned long usec) { kutox_delay_us(usec); }

#ifndef KUTOX_E1000_FULL_SOURCE
struct net_device *e1000_get_hw_dev(struct e1000_hw *hw)
{
    struct e1000_adapter *adapter = hw->back;
    if (!adapter || !adapter->netdev)
        kutox_platform_fail("log_context");
    return adapter->netdev;
}
#endif

void kutox_debug(const struct net_device *dev, const char *format, ...)
{
    if (!dev || !dev->name[0])
        kutox_platform_fail("log_context");
    printk("LINUX_E1000 %s: ", dev->name);
    va_list arguments;
    va_start(arguments, format);
    kutox_vprint(format, arguments);
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
    extern void kutox_driver_outl(u32, unsigned long);
    kutox_driver_outl(value,port);
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
#if !KUTOX_GUEST_TESTS
    if (deny_grant) {
        result->code = -95;
        return;
    }
#endif
    probe_active = true;
    mmio_base = base;
    mmio_length = length;
#if KUTOX_GUEST_TESTS
    deny_eeprom_grant = deny_grant;
    denied_grants = 0;
#endif
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
#if KUTOX_GUEST_TESTS
    result->denied_grants = denied_grants;
#else
    result->denied_grants = 0;
#endif
    result->eeprom_reads = eeprom_reads;
    probe_active = false;
    mmio_base = 0;
    mmio_length = 0;
}
