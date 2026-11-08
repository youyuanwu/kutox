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
int kutox_c_linux_info(uint64_t ticket, u8 *mac, unsigned int *mtu, unsigned int *link)
{
    if (!ticket || ticket != generation) return -ESTALE;
    if (!loaded || !device || !netif_running(device)) return -ENODEV;
    if (!mac || !mtu || !link) return -EINVAL;
    memcpy(mac,device->dev_addr,ETH_ALEN);
    *mtu = device->mtu;
    *link = netif_carrier_ok(device);
    return 0;
}
int kutox_c_linux_statistics(uint64_t ticket, uint64_t *statistics)
{
    if (!ticket || ticket != generation) return -ESTALE;
    if (!loaded || !device) return -ENODEV;
    if (!statistics) return -EINVAL;
    statistics[0] = device->stats.tx_packets;
    statistics[1] = device->stats.tx_bytes;
    statistics[2] = device->stats.rx_packets;
    statistics[3] = device->stats.rx_bytes;
    statistics[4] = device->sent_packets;
    statistics[5] = device->completed_packets;
    statistics[6] = device->stats.rx_dropped;
    return 0;
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
    if (!netif_running(device)) return -ENODEV;
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
#if KUTOX_GUEST_TESTS
int kutox_c_linux_rx_state(uint64_t ticket, unsigned int packets, unsigned int bytes,
                          unsigned int dropped, unsigned int failures, bool replenished)
{
    if (!ticket || ticket != generation || !loaded || !device) return -ESTALE;
    if (!netif_running(device)) return -ENODEV;
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
#endif
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

#if KUTOX_GUEST_TESTS
int kutox_c_linux_queue(uint64_t ticket, bool stopped)
{
    if (!ticket || ticket != generation || !loaded || !device) return -ESTALE;
    if (stopped) netif_stop_queue(device); else netif_wake_queue(device);
    return 0;
}
int kutox_c_linux_many(uint64_t ticket, const void *bytes, unsigned int count)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    return kutox_c_tx_many(device,bytes,count,60);
}
int kutox_c_linux_many_retry(uint64_t ticket)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    return kutox_c_tx_many_retry(device);
}
int kutox_c_linux_many_finish(uint64_t ticket)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    kutox_c_tx_many_finish(device); return 0;
}
int kutox_c_linux_counts(uint64_t ticket, unsigned int sent, unsigned int completed,
                        unsigned int tx_stats, unsigned int rx_stats, unsigned int rx_bytes)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    return device->sent_packets == sent && device->completed_packets == completed &&
        device->sent_bytes == sent * 60UL && device->completed_bytes == completed * 60UL &&
        device->stats.tx_packets == tx_stats && device->stats.tx_bytes == tx_stats * 60UL &&
        device->stats.rx_packets == rx_stats && device->stats.rx_bytes == rx_bytes ? 0 : -EAGAIN;
}
int kutox_c_linux_indices(uint64_t ticket, unsigned int tx, unsigned int rx)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    if (!netif_running(device)) return -ENODEV;
    struct e1000_adapter *adapter = netdev_priv(device);
    return adapter->tx_ring->next_to_use == tx && adapter->tx_ring->next_to_clean == tx &&
        adapter->rx_ring->next_to_clean == rx &&
        adapter->rx_ring->next_to_use == (rx + 255) % 256 ? 0 : -EAGAIN;
}
int kutox_c_linux_kick(uint64_t ticket, unsigned int operation)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    if (!netif_running(device)) return -ENODEV;
    struct e1000_adapter *adapter = netdev_priv(device);
    if (operation == 0) return schedule_delayed_work(&adapter->watchdog_task,2 * HZ);
    if (operation == 1) {
        cancel_delayed_work_sync(&adapter->watchdog_task);
        return schedule_delayed_work(&adapter->watchdog_task,0);
    }
    if (operation == 2) {
        if (napi_schedule_prep(&adapter->napi)) __napi_schedule(&adapter->napi);
        return 0;
    }
    if (operation == 3) return schedule_work(&adapter->reset_task);
    return -EINVAL;
}
int kutox_c_linux_live_ring_free(uint64_t ticket)
{
    extern int kutox_linux_dma_free(uintptr_t,size_t,uintptr_t,u64);
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    struct e1000_adapter *adapter = netdev_priv(device);
    struct e1000_tx_ring *ring = adapter->tx_ring;
    if (!ring->desc) return -ENODEV;
    if ((readl(adapter->hw.hw_addr + E1000_RCTL) & E1000_RCTL_EN) ||
        (readl(adapter->hw.hw_addr + E1000_TCTL) & E1000_TCTL_EN)) return -EINVAL;
    return kutox_linux_dma_free((uintptr_t)&adapter->pdev->dev,ring->size,(uintptr_t)ring->desc,ring->dma);
}
int kutox_c_linux_rx_failures(uint64_t ticket)
{
    if (!loaded || !ticket || ticket != generation || !device) return -ESTALE;
    struct e1000_adapter *adapter = netdev_priv(device);
    if (adapter->alloc_rx_buff_failed > 0x7fffffff) return -ERANGE;
    return adapter->alloc_rx_buff_failed;
}
#endif
