/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "common/kutox_compat.h"

extern void kutox_platform_fail(const char *operation) __attribute__((noreturn));
extern int kutox_linux_dma_mask(uintptr_t key, u64 mask);
extern int kutox_linux_dma_alloc(uintptr_t key, size_t size, unsigned int flags, uintptr_t *cpu, u64 *address);
extern int kutox_linux_dma_free(uintptr_t key, size_t size, uintptr_t cpu, u64 address);
static struct device test_device;

static void check(bool condition, const char *operation)
{
    if (!condition)
        kutox_platform_fail(operation);
}

uintptr_t kutox_c_dma_key(void) { return (uintptr_t)&test_device; }

void kutox_c_dma_worker(void)
{
    check(dma_set_mask_and_coherent(&test_device, DMA_BIT_MASK(64)) == 0 &&
          dma_set_mask_and_coherent(&test_device, DMA_BIT_MASK(32)) == 0, "c_dma_masks");
    check(dma_set_mask_and_coherent(&test_device, 0xffff) == -121 &&
          kutox_linux_dma_mask(0, DMA_BIT_MASK(32)) == -128, "c_dma_invalid_device_mask");
    uintptr_t cpu = 0x5eed;
    u64 address = 0x5eed;
    check(kutox_linux_dma_alloc(kutox_c_dma_key(), 0, GFP_KERNEL, &cpu, &address) == -120 &&
          cpu == 0x5eed && address == 0x5eed, "c_dma_invalid_output_unchanged");
    check(kutox_linux_dma_alloc(kutox_c_dma_key(), 16, 1, &cpu, &address) == -120 &&
          cpu == 0x5eed && address == 0x5eed, "c_dma_flags_rejected");
    check(kutox_linux_dma_alloc(kutox_c_dma_key(), 16, GFP_KERNEL, NULL, &address) == -120 &&
          address == 0x5eed, "c_dma_null_output_rejected");
    void *buffers[8];
    dma_addr_t handles[8];
    for (size_t i = 0; i < ARRAY_SIZE(buffers); i++) {
        buffers[i] = dma_alloc_coherent(&test_device, 16 + i, &handles[i], GFP_KERNEL);
        check(buffers[i] && (uintptr_t)buffers[i] != handles[i] &&
              !((uintptr_t)buffers[i] & 4095) && !(handles[i] & 4095) &&
              handles[i] + 4095 <= DMA_BIT_MASK(32), "c_dma_address_alignment");
        for (size_t byte = 0; byte < 16 + i; byte++)
            check(((u8 *)buffers[i])[byte] == 0, "c_dma_zeroed");
        for (size_t old = 0; old < i; old++)
            check(buffers[old] != buffers[i] && handles[old] != handles[i], "c_dma_disjoint");
    }
    address = 0x5eed;
    check(kutox_linux_dma_alloc(0, 16, GFP_KERNEL, &cpu, &address) == -128 &&
          cpu == 0x5eed && address == 0x5eed, "c_dma_full_registry_invalid_key");
    check(kutox_linux_dma_alloc(kutox_c_dma_key(), 0, GFP_KERNEL, &cpu, &address) == -120 &&
          cpu == 0x5eed && address == 0x5eed, "c_dma_full_registry_invalid_size");
    check(!dma_alloc_coherent(&test_device, 16, &address, GFP_KERNEL) && address == 0x5eed,
          "c_dma_capacity_output_unchanged");
    check(dma_set_mask_and_coherent(&test_device, DMA_BIT_MASK(64)) == -123 &&
          kutox_destroy_dma_device(&test_device) == -123, "c_dma_live_device_rejected");
    check(kutox_linux_dma_free(kutox_c_dma_key(), 17, (uintptr_t)buffers[0], handles[0]) == -122 &&
          kutox_linux_dma_free(kutox_c_dma_key(), 16, (uintptr_t)buffers[0], handles[0] + 1) == -122,
          "c_dma_wrong_free_rejected");
    ((u8 *)buffers[0])[0] = 0x5a;
    dma_wmb();
    check(kutox_dma_publish(&test_device, buffers[0]) == 0, "c_dma_publish");
    check(kutox_linux_dma_free(kutox_c_dma_key(), 16, (uintptr_t)buffers[0], handles[0]) == -123 &&
          kutox_dma_publish(&test_device, buffers[0]) == -123, "c_dma_live_buffer_rejected");
    check(cond_resched() == 1, "c_dma_stackful_yield");
    /* The native test never enables bus mastering or starts a DMA engine. */
    check(kutox_dma_reclaim(&test_device, buffers[0]) == 0 &&
          kutox_dma_reclaim(&test_device, buffers[0]) == -125, "c_dma_reclaim");
    dma_rmb();
    check(((u8 *)buffers[0])[0] == 0x5a, "c_dma_payload_preserved");
    uintptr_t retired = (uintptr_t)buffers[0];
    for (size_t i = 0; i < ARRAY_SIZE(buffers); i++)
        dma_free_coherent(&test_device, 16 + i, buffers[i], handles[i]);
    check(kutox_linux_dma_free(kutox_c_dma_key(), 16, retired, handles[0]) == -124,
          "c_dma_double_free_rejected");
    void *reused = dma_alloc_coherent(&test_device, 16, &address, GFP_KERNEL);
    check(reused && ((u8 *)reused)[0] == 0, "c_dma_reuse_zeroed");
    dma_free_coherent(&test_device, 16, reused, address);
}
