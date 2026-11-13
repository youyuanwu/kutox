/* SPDX-License-Identifier: GPL-2.0-only */
/* Kept in C for original e1000 private structures and variadic debug wrappers.
 * Rust owns MMIO access, probe ownership and fault-injection policy. */
#include "e1000.h"
#include "net/bridge.h"
extern int kutox_vprint(const char *, va_list);
extern void kutox_platform_fail(const char *) __attribute__((noreturn));
extern bool kutox_log_prefix(unsigned int, const void *, const char *, u32, const char *);
#ifndef KUTOX_E1000_FULL_SOURCE
struct net_device *e1000_get_hw_dev(struct e1000_hw *hw)
{
    struct e1000_adapter *a = hw->back;
    if (!a || !a->netdev) kutox_platform_fail("log_context");
    return a->netdev;
}
#endif

void kutox_debug(const struct net_device *dev, const char *format, ...)
{
    kutox_log_prefix(4, dev, NULL, 0, format);
    va_list arguments;
    va_start(arguments, format);
    kutox_vprint(format, arguments);
    va_end(arguments);
}

void kutox_e1000_mac(uintptr_t base, u16 vendor, u16 device, u8 revision,
                     struct kutox_result *result)
{
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
    if (result->code) return;
    result->stage = KUTOX_EEPROM_PARAMS;
    result->code = e1000_init_eeprom_params(hw);
    if (result->code) return;
    result->stage = KUTOX_READ_EEPROM;
    result->code = e1000_read_mac_addr(hw);
    if (result->code) return;
    memcpy(result->mac, hw->perm_mac_addr, sizeof(result->mac));
    result->stage = KUTOX_COMPLETE;
}
