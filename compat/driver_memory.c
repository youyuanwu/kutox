/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver_services.h"
void *kmalloc(size_t size, gfp_t flags) { return (void *)kutox_driver_alloc(size, flags, 0); }
void *kzalloc(size_t size, gfp_t flags) { return kmalloc(size, flags); }
void *kmalloc_array(size_t count, size_t size, gfp_t flags)
{
    if (!count || !size || count > SIZE_MAX / size) {
        kutox_driver_diag("allocation_overflow", -EINVAL); return NULL;
    }
    return kmalloc(count * size, flags);
}
void *kcalloc(size_t count, size_t size, gfp_t flags) { return kmalloc_array(count, size, flags); }
void *vmalloc(size_t size) { return kmalloc(size, GFP_KERNEL); }
void *vzalloc(size_t size) { return vmalloc(size); }
void kfree(const void *pointer) { driver_check(kutox_driver_free((uintptr_t)pointer), "kfree"); }
void vfree(const void *pointer) { kfree(pointer); }
char *strcpy(char *destination, const char *source)
{
    if (!destination || !source) kutox_platform_fail("strcpy_null");
    size_t index = 0;
    do {
        if (index == 4096) kutox_platform_fail("strcpy_bound");
        destination[index] = source[index];
    } while (source[index++]);
    return destination;
}
long strscpy(char *destination, const char *source, size_t capacity)
{
    if (!destination || !source || capacity > 4096) kutox_platform_fail("strscpy_arguments");
    if (!capacity) return -E2BIG;
    size_t index;
    for (index = 0; index < capacity - 1 && source[index]; index++) destination[index] = source[index];
    destination[index] = 0;
    return source[index] ? -E2BIG : (long)index;
}

dma_addr_t dma_map_single(struct device *device, void *pointer, size_t size, enum dma_data_direction direction)
{
    return kutox_driver_dma_map((uintptr_t)device, (uintptr_t)pointer, size, direction);
}
bool dma_mapping_error(struct device *device, dma_addr_t address)
{
    if ((uintptr_t)device != kutox_driver_device()) kutox_platform_fail("dma_error_device");
    return address == UINT64_MAX;
}
void dma_unmap_single(struct device *device, dma_addr_t address, size_t size, enum dma_data_direction direction)
{
    driver_check(kutox_driver_dma_operation((uintptr_t)device, address, size, direction, 0), "dma_unmap");
}
void dma_sync_single_for_cpu(struct device *device, dma_addr_t address, size_t size, enum dma_data_direction direction)
{
    driver_check(kutox_driver_dma_operation((uintptr_t)device, address, size, direction, 1), "dma_sync_cpu");
}
void dma_sync_single_for_device(struct device *device, dma_addr_t address, size_t size, enum dma_data_direction direction)
{
    driver_check(kutox_driver_dma_operation((uintptr_t)device, address, size, direction, 2), "dma_sync_device");
}
