/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver_services.h"
extern struct pci_dev *kutox_c_driver_endpoint(unsigned int);
extern void kutox_c_driver_endpoint_release(void);
extern void kutox_driver_fail_after(size_t);
extern size_t kutox_driver_receive(void *, size_t);
extern int kutox_linux_dma_free(uintptr_t, size_t, uintptr_t, u64);
extern int kutox_c_netdev_open(struct net_device *);
extern int kutox_c_netdev_transmit(struct net_device *, struct sk_buff *);
extern void kutox_linux_workers_stop(void);
extern int kutox_linux_context(void);
extern int kutox_linux_work(uintptr_t,u64,unsigned int,u64);

static void check(bool value, const char *operation)
{
    if (!value) kutox_platform_fail(operation);
}
static struct pci_dev *pdev;
static void *mmio;

void kutox_c_services_begin(unsigned int irq)
{
    pdev = kutox_c_driver_endpoint(irq);
    check(pci_enable_device(pdev) == 0, "p3_pci_enable");
    check(pci_select_bars(pdev,IORESOURCE_MEM) == 1 &&
          pci_request_selected_regions(pdev,1,"p3-test") == 0, "p3_pci_regions");
    pci_set_master(pdev);
    check(pci_save_state(pdev) == 0, "p3_pci_save");
    /* The original probe enables master before mapping and mask selection. */
    mmio = pci_ioremap_bar(pdev,0);
    check(mmio && dma_set_mask_and_coherent(&pdev->dev,DMA_BIT_MASK(32)) == 0,
          "p3_probe_order");
    u16 command;
    check(pci_read_config_word(pdev,4,&command) == 0 && (command & 6) == 6,
          "p3_actual_bus_master");
    check(pci_resource_len(pdev,0) == 0x20000 && pci_resource_flags(pdev,0) == IORESOURCE_MEM &&
          pci_resource_start(pdev,0) != 0 && !pci_channel_offline(pdev), "p3_pci_resources");
    check(pci_set_mwi(pdev) == -EIO, "p3_mwi_unsupported_readback"); pci_clear_mwi(pdev);
    device_set_wakeup_enable(&pdev->dev,false);
    check(!device_can_wakeup(&pdev->dev), "p3_wakeup_state");
}
void kutox_c_services_end(void)
{
    kutox_driver_gate();
    iounmap(mmio); mmio = NULL;
    pci_release_selected_regions(pdev,1); pci_disable_device(pdev);
    pci_set_drvdata(pdev,NULL);
    kutox_c_driver_endpoint_release(); pdev = NULL;
}

void kutox_c_services_allocations(void)
{
    check(GFP_KERNEL == 3264 && GFP_ATOMIC == 2080, "p3_actual_gfp");
    check(!kmalloc_array(SIZE_MAX,2,GFP_KERNEL), "p3_overflow_output");
    kutox_driver_fail_after(0);
    check(!kzalloc(32,GFP_KERNEL) && !alloc_etherdev(32), "p3_allocation_failure");
    kutox_driver_fail_after(SIZE_MAX);
    u8 *object = kzalloc(33,GFP_KERNEL);
    check(object && !((uintptr_t)object & 63), "p3_alignment");
    for (size_t i = 0; i < 33; i++) check(!object[i], "p3_zeroing");
    object[32] = 0xa5;
    check(kutox_driver_size((uintptr_t)object) == 33 &&
          kutox_driver_free((uintptr_t)object + 1) == -16, "p3_exact_free");
    kfree(object);
    check(kutox_driver_free((uintptr_t)object) == -22, "p3_double_free");
    object = vmalloc(8193); check(object, "p3_vmalloc"); vfree(object);
    kutox_driver_fail_after(1);
    check(!alloc_etherdev(32), "p3_netdev_private_failure_rollback");
    kutox_driver_fail_after(SIZE_MAX);
    void *pressure[64];
    size_t count;
    for (count = 0; count < ARRAY_SIZE(pressure); count++) {
        pressure[count] = kmalloc(4 * 1024 * 1024,GFP_KERNEL);
        if (!pressure[count]) break;
    }
    check(count && count < ARRAY_SIZE(pressure), "p3_actual_allocation_pressure");
    while (count) kfree(pressure[--count]);
    object = kmalloc(32,GFP_KERNEL); check(object, "p3_pressure_recovery"); kfree(object);
}

void kutox_c_services_dma(void)
{
    u8 *object = kmalloc(2048,GFP_KERNEL);
    check(object, "p3_stream_backing");
    dma_addr_t invalid = dma_map_single(&pdev->dev,object + 1,2048,DMA_FROM_DEVICE);
    check(dma_mapping_error(&pdev->dev,invalid), "p3_mapping_range_error");
    invalid = dma_map_single(&pdev->dev,object,2048,DMA_BIDIRECTIONAL);
    check(dma_mapping_error(&pdev->dev,invalid), "p3_mapping_direction_error");
    dma_addr_t mapped = dma_map_single(&pdev->dev,object,2048,DMA_FROM_DEVICE);
    check(!dma_mapping_error(&pdev->dev,mapped), "p3_map");
    check(dma_set_mask_and_coherent(&pdev->dev,0xffffff) == -EIO &&
          dma_set_mask_and_coherent(&pdev->dev,DMA_BIT_MASK(64)) == -EBUSY, "p3_mask_changes_live_rejected");
    check(kutox_driver_free((uintptr_t)object) == -16 &&
          kutox_driver_range((uintptr_t)object,1,0) == -16, "p3_pin_blocks_early_cpu_and_free");
    check(kutox_driver_dma_operation((uintptr_t)&pdev->dev,mapped,2047,DMA_FROM_DEVICE,0) == -22 &&
          kutox_driver_dma_operation((uintptr_t)&pdev->dev,mapped,2048,DMA_TO_DEVICE,0) == -22 &&
          kutox_driver_dma_operation(1,mapped,2048,DMA_FROM_DEVICE,0) == -19, "p3_exact_unmap");
    dma_sync_single_for_cpu(&pdev->dev,mapped,60,DMA_FROM_DEVICE);
    check(kutox_driver_range((uintptr_t)object,60,0) == 0 &&
          kutox_driver_range((uintptr_t)object + 60,1,0) == -16, "p3_partial_sync_range");
    dma_sync_single_for_device(&pdev->dev,mapped,2048,DMA_FROM_DEVICE);
    check(kutox_driver_range((uintptr_t)object,1,0) == -16, "p3_sync_device");
    dma_unmap_single(&pdev->dev,mapped,2048,DMA_FROM_DEVICE);
    check(kutox_driver_dma_operation((uintptr_t)&pdev->dev,mapped,2048,DMA_FROM_DEVICE,0) == -22,
          "p3_double_unmap");
    kfree(object);
    dma_addr_t address = 0xdead;
    kutox_driver_fail_after(0);
    check(!dma_alloc_coherent(&pdev->dev,4096,&address,GFP_KERNEL) && address == 0xdead,
          "p3_coherent_failure_output");
    kutox_driver_fail_after(SIZE_MAX);
    void *ring = dma_alloc_coherent(&pdev->dev,4096,&address,GFP_KERNEL);
    check(ring && !(address & 4095) && !((uintptr_t)ring & 4095), "p3_coherent_alignment");
    ((volatile u32 *)ring)[3] = 0x11223344;
    check(((volatile u32 *)ring)[3] == 0x11223344, "p3_protocol_shared_ring");
    check(kutox_linux_dma_free((uintptr_t)&pdev->dev,4095,(uintptr_t)ring,address) == -22,
          "p3_coherent_exact_free");
    writel(2,(u8 *)mmio + 0x100);
    check(kutox_linux_dma_free((uintptr_t)&pdev->dev,4096,(uintptr_t)ring,address) == -16,
          "p3_live_engine_free_excluded");
    check(kutox_driver_free((uintptr_t)ring) == -16, "p3_generic_free_cannot_bypass_ring_lifetime");
    kutox_driver_gate();
    check(!(readl((u8 *)mmio + 0x100) & 2), "p3_gate_stops_rx");
    dma_free_coherent(&pdev->dev,4096,ring,address);
}

static bool consume_tx;
static netdev_tx_t synthetic_xmit(struct sk_buff *skb, struct net_device *device)
{
    if (!consume_tx) return NETDEV_TX_BUSY;
    dev_kfree_skb_any(skb);
    return NETDEV_TX_OK;
}
static int synthetic_open(struct net_device *device) { netif_start_queue(device); return 0; }
static int synthetic_stop(struct net_device *device) { netif_stop_queue(device); return 0; }
static const struct net_device_ops operations = {
    .ndo_open=synthetic_open, .ndo_stop=synthetic_stop, .ndo_start_xmit=synthetic_xmit,
};
static bool atomic_allocation_seen;
static void cancelled_embedded_work(struct work_struct *work) { kutox_platform_fail("embedded_work_not_cancelled"); }
static int unused_poll(struct napi_struct *napi, int budget)
{
    check(kutox_linux_context() == -58 && !kmalloc(32,GFP_KERNEL), "p3_atomic_rejects_sleeping_gfp");
    void *pointer = kmalloc(32,GFP_ATOMIC);
    check(pointer, "p3_atomic_allocation");
    kfree(pointer);
    check(napi_complete_done(napi,0), "p3_atomic_poll_complete");
    atomic_allocation_seen = true;
    return 0;
}

void kutox_c_services_packets(void)
{
    struct net_device *device = alloc_etherdev(256);
    check(device && !((uintptr_t)netdev_priv(device) & 63), "p3_netdev_private");
    device->netdev_ops = &operations;
    struct work_struct *embedded = netdev_priv(device);
    INIT_WORK(&embedded[0],cancelled_embedded_work); INIT_WORK(&embedded[1],cancelled_embedded_work);
    check(schedule_work(&embedded[1]), "p3_embedded_work_queued");
    check(kutox_driver_free((uintptr_t)embedded) == -103 &&
          kutox_linux_work((uintptr_t)&embedded[0],embedded[0].generation,3,0) == 0,
          "p3_failed_container_free_preserves_idle_registration");
    check(cancel_work_sync(&embedded[1]), "p3_embedded_work_cancelled");
    mutex_init((struct mutex *)((u8 *)embedded + 64));
    SET_NETDEV_DEV(device,&pdev->dev); pci_set_drvdata(pdev,device);
    const u8 mac[6] = {0x52,0x54,0,0x12,0x34,0x56};
    eth_hw_addr_set(device,mac);
    check(eth_validate_addr(device) == 0 && register_netdev(device) == 0 &&
          !netif_running(device) && !netif_carrier_ok(device), "p3_netdev_registration");
    check(kutox_c_netdev_open(device) == 0 && netif_running(device) &&
          !netif_queue_stopped(device), "p3_registered_ops");
    netif_carrier_on(device); netif_device_detach(device);
    check(netif_carrier_ok(device) && netif_xmit_stopped(netdev_get_tx_queue(device,0)), "p3_queue_detach");
    netif_device_attach(device); netif_wake_queue(device);
    struct sk_buff *skb = alloc_skb(10,GFP_ATOMIC);
    check(skb, "p3_skb"); skb_put_data(skb,"KUTOX",5);
    check(eth_skb_pad(skb) == 0 && skb_headlen(skb) == 60, "p3_skb_padding");
    for (unsigned int i = 5; i < 60; i++) check(!skb->data[i], "p3_padding_zero");
    check(!skb_is_gso(skb) && !skb_vlan_tag_present(skb), "p3_packet_metadata");
    skb_checksum_none_assert(skb); skb_tx_timestamp(skb);
    check(kutox_c_netdev_transmit(device,skb) == NETDEV_TX_BUSY && skb->len == 60,
          "p3_busy_retains_packet");
    skb_shinfo(skb)->gso_size = 64;
    check(kutox_c_netdev_transmit(device,skb) == -EOPNOTSUPP && skb->len == 60, "p3_gso_rejected_retains_packet");
    skb_shinfo(skb)->gso_size = 0;
    skb->no_fcs = true;
    check(kutox_c_netdev_transmit(device,skb) == -EOPNOTSUPP, "p5_no_fcs_rejected");
    skb->no_fcs = false;
    check(kutox_c_tx_begin(device,(uintptr_t)mmio,skb->data,60) == 0, "p5_owned_packet");
    check(kutox_c_tx_retry(device) == NETDEV_TX_BUSY &&
          kutox_c_tx_status(device) == 0, "p5_busy_packet_owned");
    kutox_c_tx_release(device);
    check(kutox_c_tx_begin(device,(uintptr_t)mmio,skb->data,60) == 0, "p5_drop_packet");
    consume_tx = true;
    check(kutox_c_tx_retry(device) == NETDEV_TX_OK &&
          kutox_c_tx_status(device) == -EIO, "p5_consumed_drop_is_not_completion");
    consume_tx = false; kutox_c_tx_release(device);
    netdev_sent_queue(device,60); netdev_completed_queue(device,1,60);
    dev_kfree_skb_any(skb);
    struct napi_struct napi = {0}; netif_napi_add(device,&napi,unused_poll);
    dma_addr_t callback_ring_dma;
    void *callback_ring = dma_alloc_coherent(&pdev->dev,4096,&callback_ring_dma,GFP_KERNEL);
    check(callback_ring, "p3_callback_ring");
    napi_enable(&napi);
    check(kutox_linux_dma_free((uintptr_t)&pdev->dev,4096,(uintptr_t)callback_ring,callback_ring_dma) == -16,
          "p3_enabled_poll_blocks_ring_free");
    check(napi_schedule_prep(&napi), "p3_atomic_poll_prepare"); __napi_schedule(&napi);
    for (size_t attempt = 0; attempt < 100 && !atomic_allocation_seen; attempt++) msleep(1);
    check(atomic_allocation_seen, "p3_atomic_allocation_executed");
    napi_disable(&napi); atomic_allocation_seen = false;
    dma_free_coherent(&pdev->dev,4096,callback_ring,callback_ring_dma);
    u8 frame[60] = {0};
    memset(frame,255,6); memcpy(frame + 6,mac,6); frame[12] = 0x88; frame[13] = 0xb5;
    memcpy(frame + 14,"KUTOX-SOURCE-SERVICES",20);
    skb = napi_alloc_skb(&napi,60); check(skb, "p3_copybreak_alloc");
    skb_put_data(skb,frame,60);
    check(eth_type_trans(skb,device) == htons(0x88b5) && skb->len == 46 &&
          napi_gro_receive(&napi,skb) == 0, "p3_ingress_protocol");
    u8 received[1514];
    check(kutox_driver_receive(received,sizeof(received)) == 60, "p3_owned_ingress");
    for (size_t i = 0; i < 60; i++) check(received[i] == frame[i], "p3_exact_ingress");
    unsigned int frag_size = SKB_DATA_ALIGN(2048 + 66) + SKB_DATA_ALIGN(sizeof(struct skb_shared_info));
    void *fragment = netdev_alloc_frag(frag_size); check(fragment, "p3_fragment_alloc");
    memcpy((u8 *)fragment + 66,frame,60);
    dma_addr_t dma = dma_map_single(&pdev->dev,(u8 *)fragment + 66,2048,DMA_FROM_DEVICE);
    check(!dma_mapping_error(&pdev->dev,dma), "p3_fragment_map");
    skb = napi_build_skb(fragment,frag_size); check(skb, "p3_build_skb_before_unmap");
    skb_reserve(skb,66);
    dma_unmap_single(&pdev->dev,dma,2048,DMA_FROM_DEVICE);
    skb_put(skb,60); eth_type_trans(skb,device); napi_gro_receive(&napi,skb);
    check(kutox_driver_receive(received,sizeof(received)) == 60, "p3_build_handoff_ingress");
    for (size_t i = 0; i < 60; i++) check(received[i] == frame[i], "p3_build_handoff_bytes");
    fragment = netdev_alloc_frag(frag_size);
    skb_free_frag((u8 *)fragment + 66); /* Original normal ring cleanup passes this interior pointer. */
    napi_free_frags(&napi); netif_napi_del(&napi);
    unregister_netdev(device); free_netdev(device);
    kutox_linux_workers_stop();
}

void kutox_c_services_capacity(void)
{
    void *rx[256]; dma_addr_t mappings[256]; void *tx[256]; dma_addr_t tx_mappings[256];
    dma_addr_t tx_dma, rx_dma;
    void *tx_ring = dma_alloc_coherent(&pdev->dev,4096,&tx_dma,GFP_KERNEL);
    void *rx_ring = dma_alloc_coherent(&pdev->dev,4096,&rx_dma,GFP_KERNEL);
    check(tx_ring && rx_ring, "p3_default_rings");
    unsigned int frag_size = SKB_DATA_ALIGN(2048 + 66) + SKB_DATA_ALIGN(sizeof(struct skb_shared_info));
    for (size_t i = 0; i < 256; i++) {
        rx[i] = netdev_alloc_frag(frag_size);
        tx[i] = kmalloc(1514,GFP_ATOMIC);
        check(rx[i] && tx[i], "p3_256_buffer_capacity");
        mappings[i] = dma_map_single(&pdev->dev,(u8 *)rx[i]+66,2048,DMA_FROM_DEVICE);
        tx_mappings[i] = dma_map_single(&pdev->dev,tx[i],1514,DMA_TO_DEVICE);
        check(!dma_mapping_error(&pdev->dev,mappings[i]) && !dma_mapping_error(&pdev->dev,tx_mappings[i]),
              "p3_512_mapping_capacity");
    }
    writel(2,(u8 *)mmio + 0x100);
    dma_unmap_single(&pdev->dev,tx_mappings[0],1514,DMA_TO_DEVICE);
    check((readl((u8 *)mmio + 0x100) & 2) &&
          kutox_driver_free((uintptr_t)rx[0]) == -16, "p3_tx_unmap_preserves_rx");
    kfree(tx[0]); tx[0] = NULL;
    kutox_driver_gate();
    for (size_t i = 0; i < 256; i++) {
        dma_unmap_single(&pdev->dev,mappings[i],2048,DMA_FROM_DEVICE);
        if (tx[i]) dma_unmap_single(&pdev->dev,tx_mappings[i],1514,DMA_TO_DEVICE);
        skb_free_frag((u8 *)rx[i]+66); kfree(tx[i]);
    }
    dma_free_coherent(&pdev->dev,4096,tx_ring,tx_dma);
    dma_free_coherent(&pdev->dev,4096,rx_ring,rx_dma);
}
static irqreturn_t unused_irq(int irq, void *device) { return IRQ_NONE; }
void kutox_c_services_failed_open(void)
{
    void *rx = netdev_alloc_frag(2496);
    dma_addr_t dma = dma_map_single(&pdev->dev,(u8 *)rx+66,2048,DMA_FROM_DEVICE);
    check(rx && !dma_mapping_error(&pdev->dev,dma), "p3_failed_open_rx");
    writel(2,(u8 *)mmio + 0x100);
    check(request_irq(100,unused_irq,IRQF_SHARED,"failed-open",pdev) < 0, "p3_irq_failure");
    check(!(readl((u8 *)mmio + 0x100) & 2), "p3_irq_failure_gated_before_return");
    dma_unmap_single(&pdev->dev,dma,2048,DMA_FROM_DEVICE); skb_free_frag((u8 *)rx+66);
}
