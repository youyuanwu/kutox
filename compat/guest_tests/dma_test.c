/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "common/kutox_compat.h"

/* Exercise the declarations and macros used by C drivers, not native backends. */
bool kutox_c_dma_boundary(struct device *device, bool exhausted)
{
    if (exhausted) {
        dma_addr_t unchanged = 0x5eed;
        return !dma_alloc_coherent(device, 16, &unchanged, GFP_KERNEL) && unchanged == 0x5eed;
    }
    if (dma_set_mask_and_coherent(device, DMA_BIT_MASK(32)) != 0)
        return false;
    dma_addr_t address;
    u8 *buffer = dma_alloc_coherent(device, 32, &address, GFP_KERNEL);
    if (!buffer)
        return false;
    bool valid = !((uintptr_t)buffer & 4095) && !(address & 4095) &&
                 (uintptr_t)buffer != address && address + 4095 <= DMA_BIT_MASK(32);
    for (size_t i = 0; i < 32; i++)
        valid = valid && buffer[i] == 0;
    buffer[0] = 0x5a;
    dma_wmb();
    dma_rmb();
    valid = valid && buffer[0] == 0x5a;
    dma_free_coherent(device, 32, buffer, address);
    return valid;
}
