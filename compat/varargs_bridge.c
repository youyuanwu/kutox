/* SPDX-License-Identifier: GPL-2.0-only */
/* Kept in C for compiler-owned va_list and variadic calling conventions.
 * Rust owns formatting, output sinks and warning policy. */
#include "common/kutox_compat.h"
#include "common/diagnostics_services.h"
extern int kutox_format(char *, size_t, const char *, void *, bool);
extern bool kutox_log_prefix(unsigned int, const void *, const char *, u32, const char *);

u64 kutox_va_unsigned(void *args, unsigned int longs)
{
    va_list *a = args;
    return longs == 2 ? va_arg(*a, unsigned long long) :
        longs == 1 ? va_arg(*a, unsigned long) : va_arg(*a, unsigned int);
}
s64 kutox_va_signed(void *args, unsigned int longs)
{
    va_list *a = args;
    return longs == 2 ? va_arg(*a, long long) :
        longs == 1 ? va_arg(*a, long) : va_arg(*a, int);
}
const void *kutox_va_pointer(void *args)
{
    va_list *a = args;
    return va_arg(*a, const void *);
}
int kutox_vprint(const char *format, va_list args)
{
    va_list copy;
    va_copy(copy, args);
    int n = kutox_format(NULL, 0, format, &copy, true);
    va_end(copy);
    return n;
}
int kutox_vformat(char *buffer, size_t capacity, const char *format, va_list args)
{
    va_list copy;
    va_copy(copy, args);
    int n = kutox_format(buffer, capacity, format, &copy, false);
    va_end(copy);
    return n;
}
int printk(const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int n = kutox_vprint(format, args);
    va_end(args);
    return n;
}
void netdev_printk(const struct net_device *device, const char *format, ...)
{
    kutox_log_prefix(0, device, NULL, 0, format);
    va_list args;
    va_start(args, format); kutox_vprint(format, args); va_end(args);
}
void dev_printk(const struct device *device, const char *format, ...)
{
    kutox_log_prefix(1, device, NULL, 0, format);
    va_list args;
    va_start(args, format); kutox_vprint(format, args); va_end(args);
}
void netdev_warn_once(const struct net_device *device, const char *format, ...)
{
    if (!kutox_log_prefix(2, device, NULL, 0, format)) return;
    va_list args;
    va_start(args, format); kutox_vprint(format, args); va_end(args);
}
void netif_log(u32 mask, const char *level, const struct net_device *device, const char *format, ...)
{
    if (!kutox_log_prefix(3, device, level, mask, format)) return;
    va_list args;
    va_start(args, format); kutox_vprint(format, args); va_end(args);
}
