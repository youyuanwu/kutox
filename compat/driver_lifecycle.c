/* SPDX-License-Identifier: GPL-2.0-only */
#include "net/services.h"
#include "net/accessors.h"
#include "e1000.h"

void kutox_e1000_view(struct net_device *device, struct kutox_e1000_view *out)
{
    struct e1000_adapter *a = netdev_priv(device);
    *out = (struct kutox_e1000_view){
        .mmio=(uintptr_t)a->hw.hw_addr,
        .tx_count=a->tx_ring->count, .rx_count=a->rx_ring->count,
        .rx_buffer_len=a->rx_buffer_len,
        .rx_backing_size=SKB_DATA_ALIGN(a->rx_buffer_len + NET_SKB_PAD + NET_IP_ALIGN) +
            SKB_DATA_ALIGN(sizeof(struct skb_shared_info)),
        .rx_failures=a->alloc_rx_buff_failed,
        .clean_rx=(uintptr_t)a->clean_rx, .alloc_rx=(uintptr_t)a->alloc_rx_buf,
        .watchdog=&a->watchdog_task.work, .reset=&a->reset_task, .napi=&a->napi,
        .tx_use=a->tx_ring->next_to_use, .tx_clean=a->tx_ring->next_to_clean,
        .rx_use=a->rx_ring->next_to_use, .rx_clean=a->rx_ring->next_to_clean,
        .rx_unused=E1000_DESC_UNUSED(a->rx_ring),
        .tx_desc=a->tx_ring->desc, .tx_size=a->tx_ring->size, .tx_dma=a->tx_ring->dma,
        .pci_device=&a->pdev->dev,
    };
}
#if KUTOX_GUEST_TESTS
int kutox_e1000_rx_resident(struct net_device *device)
{
    struct e1000_adapter *a = netdev_priv(device);
    unsigned int resident = 0;
    for (unsigned int i = 0; i < a->rx_ring->count; i++) {
        struct e1000_rx_buffer *buffer = &a->rx_ring->buffer_info[i];
        if (!!buffer->dma != !!buffer->rxbuf.data) kutox_platform_fail("rx_backing_pair");
        resident += buffer->dma != 0;
    }
    return resident;
}
#endif
