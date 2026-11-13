/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest test unit requires guest-tests"
#endif
#include "net/driver.h"

/* Preserve C netdev callbacks and packet macros; Rust owns service scenarios. */
static bool consume_tx;
static netdev_tx_t transmit(struct sk_buff *skb, struct net_device *device)
{
    if (!consume_tx) return NETDEV_TX_BUSY;
    dev_kfree_skb_any(skb);
    return NETDEV_TX_OK;
}
static int open_device(struct net_device *device) { netif_start_queue(device); return 0; }
static int stop_device(struct net_device *device) { netif_stop_queue(device); return 0; }
static const struct net_device_ops operations = {
    .ndo_open=open_device, .ndo_stop=stop_device, .ndo_start_xmit=transmit,
};
bool kutox_c_packet_boundary(struct net_device *device, struct sk_buff *skb, bool consume)
{
    consume_tx = consume;
    device->netdev_ops = &operations;
    if ((uintptr_t)netdev_priv(device) & 63)
        return false;
    if (!skb)
        return true;
    skb_checksum_none_assert(skb);
    skb_tx_timestamp(skb);
    return skb_headlen(skb) == 60 && !skb_is_gso(skb) && !skb_vlan_tag_present(skb) &&
           skb_shinfo(skb)->nr_frags == 0;
}
