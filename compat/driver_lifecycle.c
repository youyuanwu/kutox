/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver_services.h"
#include "e1000.h"
extern int (*const kutox_module_init)(void);
extern void (*const kutox_module_exit)(void);
extern struct pci_dev *kutox_c_driver_endpoint(unsigned int);
extern void kutox_c_driver_endpoint_release(void);
extern struct net_device *kutox_c_registered_netdev(void);
extern int kutox_c_netdev_open(struct net_device *);
extern int kutox_c_netdev_close(struct net_device *);
extern bool kutox_c_packet_empty(void);
extern bool kutox_c_deferred_empty(void);
extern void kutox_driver_trace(unsigned int,uintptr_t,uintptr_t,int);
extern void kutox_linux_static_objects_retire(void);
static bool loaded;
static unsigned int bound_irq;
static struct net_device *device;
static uint64_t generation;

void kutox_c_linux_bind_irq(unsigned int irq) { bound_irq = irq; }
int kutox_c_linux_bind(uint64_t *ticket)
{
    if (!ticket || generation == UINT64_MAX) return -EINVAL;
    if (loaded || device) return -EBUSY;
    kutox_c_driver_endpoint(bound_irq);
    kutox_driver_trace(1,(uintptr_t)kutox_module_init,0,0);
    int result = kutox_module_init();
    if (result < 0) {
        kutox_linux_static_objects_retire();
        kutox_c_driver_endpoint_release();
        return result;
    }
    loaded = true;
    device = kutox_c_registered_netdev();
    if (!device || !device->ethtool_ops) kutox_platform_fail("linux_registered_device");
    kutox_driver_trace(33,(uintptr_t)device->netdev_ops->ndo_start_xmit,(uintptr_t)device,0);
    *ticket = ++generation;
    return 0;
}
int kutox_c_linux_open(uint64_t ticket)
{
    if (!ticket || ticket != generation) return -ESTALE;
    if (!loaded || !device) return -ENODEV;
    kutox_driver_trace(6,(uintptr_t)device->netdev_ops->ndo_open,(uintptr_t)device,0);
    return kutox_c_netdev_open(device);
}
int kutox_c_linux_ready(uint64_t ticket)
{
    if (!ticket || ticket != generation || !loaded) return -ESTALE;
    if (!device) return -ENODEV;
    struct e1000_adapter *adapter = netdev_priv(device);
    if (adapter->tx_ring->count != 256 || adapter->rx_ring->count != 256 ||
        device->features != 0 || device->mtu != 1500) kutox_platform_fail("linux_default_profile");
    return netif_running(device) && netif_carrier_ok(device) && !netif_queue_stopped(device);
}
bool kutox_c_linux_rx_failure_seen(uint64_t ticket)
{
    if (!ticket || ticket != generation || !device) kutox_platform_fail("linux_rx_snapshot_handle");
    struct e1000_adapter *adapter = netdev_priv(device);
    return adapter->alloc_rx_buff_failed != 0;
}
int kutox_c_linux_close(uint64_t ticket)
{
    if (!ticket || ticket != generation) return -ESTALE;
    if (!loaded || !device) return -ENODEV;
    kutox_c_tx_abort(device);
    kutox_driver_trace(7,(uintptr_t)device->netdev_ops->ndo_stop,(uintptr_t)device,0);
    return kutox_c_netdev_close(device);
}
int kutox_c_linux_tx_begin(uint64_t ticket, const void *bytes, unsigned int size)
{
    if (!ticket || ticket != generation || !loaded) return -ESTALE;
    if (!device) return -ENODEV;
    struct e1000_adapter *adapter = netdev_priv(device);
    return kutox_c_tx_begin(device,(uintptr_t)adapter->hw.hw_addr,bytes,size);
}
int kutox_c_linux_tx(uint64_t ticket, bool retry)
{
    if (!ticket || ticket != generation || !loaded) return -ESTALE;
    if (!device) return -ENODEV;
    return retry ? kutox_c_tx_retry(device) : kutox_c_tx_status(device);
}
int kutox_c_linux_tx_release(uint64_t ticket)
{
    if (!ticket || ticket != generation || !loaded) return -ESTALE;
    if (!device) return -ENODEV;
    kutox_c_tx_release(device);
    return 0;
}
int kutox_c_linux_rx_start(uint64_t ticket)
{
    if (!ticket || ticket != generation || !loaded || !device) return -ESTALE;
    if (!kutox_c_linux_ready(ticket)) return -EBUSY;
    struct e1000_adapter *adapter = netdev_priv(device);
    if (adapter->rx_buffer_len != MAXIMUM_ETHERNET_VLAN_SIZE ||
        SKB_DATA_ALIGN(adapter->rx_buffer_len + NET_SKB_PAD + NET_IP_ALIGN) +
        SKB_DATA_ALIGN(sizeof(struct skb_shared_info)) != 1920) kutox_platform_fail("rx_buffer_profile");
    kutox_c_rx_start(device,(uintptr_t)adapter->hw.hw_addr);
    kutox_driver_trace(34,(uintptr_t)adapter->clean_rx,(uintptr_t)device,0);
    kutox_driver_trace(35,(uintptr_t)adapter->alloc_rx_buf,(uintptr_t)device,0);
    return 0;
}
int kutox_c_linux_rx_stop(uint64_t ticket)
{
    if (!ticket || ticket != generation || !loaded || !device) return -ESTALE;
    kutox_c_rx_stop(); return 0;
}
int kutox_c_linux_rx_path(uint64_t ticket)
{
    if (!ticket || ticket != generation || !loaded || !device) return -ESTALE;
    return kutox_c_rx_path();
}
int kutox_c_linux_rx_state(uint64_t ticket, unsigned int packets, unsigned int bytes,
                          unsigned int dropped, unsigned int failures, bool replenished)
{
    if (!ticket || ticket != generation || !loaded || !device) return -ESTALE;
    struct e1000_adapter *adapter = netdev_priv(device);
    if (device->stats.rx_packets != packets || device->stats.rx_bytes != bytes ||
        device->stats.rx_dropped != dropped) return -EAGAIN;
    if (adapter->alloc_rx_buff_failed != failures ||
        (replenished && E1000_DESC_UNUSED(adapter->rx_ring))) return -EIO;
    unsigned int resident = 0;
    for (unsigned int i = 0; i < adapter->rx_ring->count; i++) {
        struct e1000_rx_buffer *buffer = &adapter->rx_ring->buffer_info[i];
        if (!!buffer->dma != !!buffer->rxbuf.data) kutox_platform_fail("rx_backing_pair");
        resident += buffer->dma != 0;
    }
    return resident;
}
int kutox_c_linux_unbind(uint64_t ticket)
{
    if (!loaded || !ticket || ticket != generation) return -ESTALE;
    kutox_driver_trace(2,(uintptr_t)kutox_module_exit,0,0);
    kutox_module_exit();
    device = NULL; loaded = false;
    kutox_linux_static_objects_retire();
    kutox_c_driver_endpoint_release();
    return 0;
}
bool kutox_c_linux_empty(void) { return !loaded && !device && kutox_c_packet_empty() && kutox_c_deferred_empty(); }
