/* SPDX-License-Identifier: GPL-2.0-only */
#include "e1000.h"

_Static_assert(sizeof(void *) == 8 && sizeof(dma_addr_t) == 8, "x86_64 addresses");
_Static_assert(sizeof(struct e1000_tx_desc) == 16, "original TX descriptor");
_Static_assert(sizeof(struct e1000_rx_desc) == 16, "original RX descriptor");
_Static_assert(offsetof(struct e1000_tx_desc, upper) == 12, "original TX status");
_Static_assert(offsetof(struct e1000_rx_desc, status) == 12, "original RX status");
_Static_assert(sizeof(struct kutox_parameter) == 48, "parameter metadata ABI");
_Static_assert(offsetof(struct kutox_parameter, count) == 24, "parameter count");

const u64 kutox_driver_layout[] __attribute__((used, section(".kutox.layouts"))) = {
    sizeof(void *), sizeof(dma_addr_t), sizeof(struct e1000_tx_desc),
    sizeof(struct e1000_rx_desc), E1000_DEFAULT_TXD, E1000_DEFAULT_RXD,
    sizeof(struct e1000_adapter), sizeof(struct e1000_tx_ring),
    sizeof(struct e1000_rx_ring), sizeof(struct e1000_tx_buffer),
    sizeof(struct e1000_rx_buffer), sizeof(struct net_device), sizeof(struct sk_buff),
    sizeof(struct skb_shared_info), E1000_RXBUFFER_2048, NET_SKB_PAD, NET_IP_ALIGN,
    SKB_DATA_ALIGN(E1000_RXBUFFER_2048 + NET_SKB_PAD + NET_IP_ALIGN) +
        SKB_DATA_ALIGN(sizeof(struct skb_shared_info)),
    PAGE_SIZE, HZ, sizeof(struct kutox_parameter), GFP_KERNEL, GFP_ATOMIC,
};
