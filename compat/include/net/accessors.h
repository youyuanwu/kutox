/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_NET_ACCESSORS_H
#define KUTOX_NET_ACCESSORS_H
struct kutox_e1000_view {
    uintptr_t mmio;
    unsigned int tx_count, rx_count, rx_buffer_len, rx_backing_size, rx_failures;
    uintptr_t clean_rx, alloc_rx;
    struct work_struct *watchdog, *reset;
    struct napi_struct *napi;
    unsigned int tx_use, tx_clean, rx_use, rx_clean, rx_unused;
    void *tx_desc;
    unsigned int tx_size;
    u64 tx_dma;
    struct device *pci_device;
};
#endif
