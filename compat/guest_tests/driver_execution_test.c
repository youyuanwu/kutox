/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "common/kutox_compat.h"
extern void kutox_platform_fail(const char *) __attribute__((noreturn));

/* C-only macro and variadic calling-convention coverage. Service scenarios are Rust. */
static void check(bool value, const char *operation)
{
    if (!value) kutox_platform_fail(operation);
}
extern int kutox_vformat(char *, size_t, const char *, va_list);
extern int storage_snprintf(char *, size_t, const char *, ...);
static void check_forwarded_list(const char *format, ...)
{
    char first[128], second[128];
    const char expected[] = "-7/42/-17/4294967296/18446744073709551615/0000000000001234/spill/Z/-123/99";
    va_list args, copy;
    va_start(args, format);
    int a = kutox_vformat(first, sizeof(first), format, args);
    int b = kutox_vformat(second, sizeof(second), format, args);
    check(a == sizeof(expected) - 1 && b == a, "c_rust_va_list_repeat_length");
    for (size_t i = 0; i < sizeof(expected); i++)
        check(first[i] == expected[i] && second[i] == expected[i], "c_rust_va_list_repeat_bytes");
    va_copy(copy, args);
    check(va_arg(args, int) == -7 && va_arg(copy, int) == -7 &&
          va_arg(args, unsigned int) == 42 && va_arg(copy, unsigned int) == 42,
          "c_rust_va_list_cursor_preserved");
    va_end(copy);
    va_end(args);
}
static int print_preserving_list(const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int n = kutox_vprint(format, args);
    check(va_arg(args, int) == -7, "c_rust_vprint_cursor_preserved");
    va_end(args);
    return n;
}
static int format_buffer(char *out, size_t capacity, const char *format, ...)
{
    va_list args;
    va_start(args, format);
    int n = kutox_vformat(out, capacity, format, args);
    va_end(args);
    return n;
}
static void check_formatting(void)
{
    check_forwarded_list("%d/%u/%ld/%llu/%zu/%p/%s/%c/%d/%u",
                         -7, 42U, -17L, 4294967296ULL, SIZE_MAX, (void *)0x1234,
                         "spill", 'Z', -123, 99U);
    char text[96];
    const char expected[] = "-9223372036854775808/-17/18446744073709551615/4294967296/x  /%";
    int n = format_buffer(text, sizeof(text), "%lld/%ld/%zu/%lu/%-3s/%%",
                         -9223372036854775807LL - 1, -17L, SIZE_MAX, 4294967296UL, "x");
    check(n == sizeof(expected) - 1, "c_rust_format_mixed_length");
    for (size_t i = 0; i < sizeof(expected); i++)
        check(text[i] == expected[i], "c_rust_format_mixed_bytes");
    n = storage_snprintf(text, sizeof(text), "%lld/%ld/%zu/%lu/%-3s/%%",
                         -9223372036854775807LL - 1, -17L, SIZE_MAX, 4294967296UL, "x");
    check(n == sizeof(expected) - 1, "c_rust_variadic_definition_length");
    for (size_t i = 0; i < sizeof(expected); i++)
        check(text[i] == expected[i], "c_rust_variadic_definition_bytes");
    struct { char text[5]; char guard; } short_output = {.guard='Q'};
    check(format_buffer(short_output.text, sizeof(short_output.text), "%s", "abcdef") == 6 &&
          short_output.text[0] == 'a' && short_output.text[3] == 'd' &&
          short_output.text[4] == 0 && short_output.guard == 'Q', "c_rust_format_truncation");
    char untouched = 'Q';
    check(format_buffer(&untouched, 0, "%llu", UINT64_MAX) == 20 && untouched == 'Q',
          "c_rust_format_zero_capacity");
    check(format_buffer(&untouched, 1, "%d", -1) == 2 && untouched == 0,
          "c_rust_format_nul_capacity");
}

void kutox_c_driver_atomics(void)
{
    check_formatting();
    atomic_t counter;
    atomic_set(&counter, -7);
    check(atomic_read(&counter) == -7, "driver_atomic_signed");
    u8 one = 0; u16 two = 0; u32 four = 0; u64 eight = 0;
    smp_store_release(&one, 0xa5); smp_store_release(&two, 0x55aa);
    smp_store_release(&four, 0xdeadbeef); smp_store_release(&eight, UINT64_MAX);
    check(smp_load_acquire(&one) == 0xa5 && smp_load_acquire(&two) == 0x55aa &&
          smp_load_acquire(&four) == 0xdeadbeef && smp_load_acquire(&eight) == UINT64_MAX,
          "driver_acquire_release_widths");
    WRITE_ONCE(one, 7); WRITE_ONCE(two, 8); WRITE_ONCE(four, 9); WRITE_ONCE(eight, 10);
    check(READ_ONCE(one) == 7 && READ_ONCE(two) == 8 && READ_ONCE(four) == 9 &&
          READ_ONCE(eight) == 10, "driver_once_widths");
    unsigned long bitmap[64] = {0};
    set_bit(63, bitmap); set_bit(64, bitmap); set_bit(4095, bitmap);
    check(!test_and_set_bit(0, bitmap) && test_and_set_bit(0, bitmap) &&
          find_first_bit(bitmap, 4096) == 0 && find_next_bit(bitmap, 4096, 1) == 63 &&
          find_next_bit(bitmap, 4096, 65) == 4095, "driver_bitmap_boundaries");
    clear_bit(63, bitmap); clear_bit(64, bitmap); clear_bit(4095, bitmap); clear_bit(0, bitmap);
    check(find_first_bit(bitmap, 4096) == 4096 && !test_bit(4095, bitmap), "driver_bitmap_clear");
    smp_mb(); smp_rmb(); smp_wmb(); smp_mb__before_atomic(); smp_mb__after_atomic();
    dma_wmb(); dma_rmb(); barrier(); cpu_relax();
    spinlock_t lock = {0};
    spin_lock_init(&lock); spin_lock(&lock); spin_unlock(&lock);
    check(time_after(2, ~0UL - 2) && time_before(~0UL - 2, 2) &&
          time_after_eq(3, 3) && !time_after(3, 3), "driver_jiffies_wrap");
    unsigned long before = jiffies;
    udelay(3000);
    check(jiffies - before >= 2, "driver_elapsed_not_irq_count");
    check(fls(0) == 0 && fls(1) == 1 && fls(0x80000000) == 32, "driver_fls");
    u16 endian = 0x55aa; cpu_to_le16s(&endian); le16_to_cpus(&endian);
    check(endian == 0x55aa, "driver_endian");
    prefetch(bitmap);
    check(netif_msg_init(-1, 7) == 7 && netif_msg_init(32, 7) == 7 &&
          netif_msg_init(3, 0) == 7 && netif_msg_init(0, 7) == 0 &&
          netif_msg_hw(1U << 13) && !netif_msg_hw(1) &&
          netif_msg_tx_done(1U << 10) && netif_msg_rx_status(1U << 11), "driver_message_masks");
    check(!warn_on(false, "not_reached"), "driver_warn_false"); bug_on(false);
    const u8 mac[6] = {0x12,0x34,0x56,0x78,0x9a,0xbc};
    check(print_preserving_list("# linux format=%d/%08X/%llu/%pM/%-3s\n",
                 -7, 42U, 4294967296ULL, mac, "end") == 60, "driver_format_count");
    struct net_device device = { .name = "synthetic" };
    netif_log(0, "drv", &device, "must not emit\n");
    netdev_warn_once(&device, "# synthetic warning once\n");
    netdev_warn_once(&device, "# synthetic warning once\n");
    unsigned int allowed = 0;
    for (unsigned int i = 0; i < 11; i++) allowed += net_ratelimit();
    check(allowed == 10, "driver_rate_limit");
}
