/* SPDX-License-Identifier: GPL-2.0-only */
#if defined(KUTOX_STORAGE_ABI_PROFILE)
#include "block/driver.h"
#include <uapi/linux/virtio_config.h>
#include <uapi/linux/virtio_ring.h>
#include <uapi/linux/virtio_blk.h>
#elif defined(KUTOX_DRIVER_ABI_PROFILE)
#include "net/driver.h"
#else
#include "common/kutox_compat.h"
#endif
#if defined(KUTOX_STORAGE_ABI_PROFILE)
#define SZ(t) sizeof(t), _Alignof(t)
#define OF(t,f) offsetof(t,f)
const unsigned long kutox_block_layout[] __attribute__((used,section(".kutox.block_layouts"))) = {
    SZ(struct page), OF(struct page,address), OF(struct page,length),
    SZ(struct scatterlist), OF(struct scatterlist,page_link), OF(struct scatterlist,offset),
    OF(struct scatterlist,length), OF(struct scatterlist,dma_address), OF(struct scatterlist,dma_length),
    SZ(struct sg_table), OF(struct sg_table,sgl), OF(struct sg_table,nents), OF(struct sg_table,orig_nents),
    SZ(struct request_queue), OF(struct request_queue,queuedata), OF(struct request_queue,kutox_core),
    SZ(struct bio_vec), OF(struct bio_vec,bv_page), OF(struct bio_vec,bv_len), OF(struct bio_vec,bv_offset),
    sizeof(((struct bio *)0)->bi_iter), _Alignof(typeof(((struct bio *)0)->bi_iter)),
    OF(typeof(((struct bio *)0)->bi_iter),bi_sector), OF(typeof(((struct bio *)0)->bi_iter),bi_size),
    SZ(struct bio), OF(struct bio,bi_iter), OF(struct bio,bi_next),
    SZ(struct request), OF(struct request,q), OF(struct request,cmd_flags), OF(struct request,rq_flags),
    OF(struct request,__sector), OF(struct request,mq_hctx), OF(struct request,rq_next),
    OF(struct request,bio), OF(struct request,special_vec),
    SZ(struct blk_mq_hw_ctx), OF(struct blk_mq_hw_ctx,queue), OF(struct blk_mq_hw_ctx,queue_num), OF(struct blk_mq_hw_ctx,driver_data),
    SZ(struct blk_mq_queue_data), OF(struct blk_mq_queue_data,rq), OF(struct blk_mq_queue_data,last),
    SZ(struct blk_mq_queue_map), OF(struct blk_mq_queue_map,nr_queues), OF(struct blk_mq_queue_map,queue_offset),
    SZ(struct blk_mq_tag_set), OF(struct blk_mq_tag_set,ops), OF(struct blk_mq_tag_set,nr_hw_queues),
    OF(struct blk_mq_tag_set,queue_depth), OF(struct blk_mq_tag_set,cmd_size), OF(struct blk_mq_tag_set,flags),
    OF(struct blk_mq_tag_set,nr_maps), OF(struct blk_mq_tag_set,numa_node), OF(struct blk_mq_tag_set,reserved_tags),
    OF(struct blk_mq_tag_set,driver_data), OF(struct blk_mq_tag_set,map), OF(struct blk_mq_tag_set,kutox_core),
    SZ(struct rq_list), OF(struct rq_list,head),
    SZ(struct io_comp_batch), OF(struct io_comp_batch,nr), OF(struct io_comp_batch,req_list),
    SZ(struct blk_mq_ops), OF(struct blk_mq_ops,queue_rq), OF(struct blk_mq_ops,queue_rqs),
    OF(struct blk_mq_ops,commit_rqs), OF(struct blk_mq_ops,complete), OF(struct blk_mq_ops,init_request),
    OF(struct blk_mq_ops,poll), OF(struct blk_mq_ops,map_queues),
    SZ(struct queue_limits), OF(struct queue_limits,logical_block_size), OF(struct queue_limits,physical_block_size),
    OF(struct queue_limits,io_min), OF(struct queue_limits,io_opt), OF(struct queue_limits,alignment_offset),
    OF(struct queue_limits,max_hw_sectors), OF(struct queue_limits,max_segments), OF(struct queue_limits,max_segment_size),
    OF(struct queue_limits,max_discard_sectors), OF(struct queue_limits,max_hw_discard_sectors), OF(struct queue_limits,max_discard_segments),
    OF(struct queue_limits,discard_granularity), OF(struct queue_limits,discard_alignment), OF(struct queue_limits,max_write_zeroes_sectors),
    OF(struct queue_limits,max_secure_erase_sectors), OF(struct queue_limits,features),
    SZ(struct gendisk), OF(struct gendisk,major), OF(struct gendisk,first_minor), OF(struct gendisk,minors),
    OF(struct gendisk,disk_name), OF(struct gendisk,private_data), OF(struct gendisk,fops), OF(struct gendisk,queue), OF(struct gendisk,dev),
    SZ(struct block_device_operations), OF(struct block_device_operations,owner), OF(struct block_device_operations,getgeo),
    OF(struct block_device_operations,free_disk), OF(struct block_device_operations,report_zones),
    BLK_STS_OK, BLK_STS_NOTSUPP, BLK_STS_TIMEOUT, BLK_STS_IOERR, BLK_STS_RESOURCE, BLK_STS_DEV_RESOURCE,
    BLK_STS_ZONE_OPEN_RESOURCE, BLK_STS_ZONE_ACTIVE_RESOURCE, REQ_OP_READ, REQ_OP_MASK, BLK_FEAT_ZONED,
    4096, SECTOR_SIZE, HCTX_MAX_TYPES,
    BIT_ULL(VIRTIO_F_VERSION_1) | BIT_ULL(VIRTIO_F_ACCESS_PLATFORM) | BIT_ULL(VIRTIO_BLK_F_RO),
    BIT_ULL(VIRTIO_F_RING_PACKED) | BIT_ULL(VIRTIO_RING_F_INDIRECT_DESC) | BIT_ULL(VIRTIO_F_ADMIN_VQ) |
        BIT_ULL(VIRTIO_BLK_F_ZONED) | BIT_ULL(VIRTIO_BLK_F_CONFIG_WCE) | BIT_ULL(VIRTIO_BLK_F_DISCARD) |
        BIT_ULL(VIRTIO_BLK_F_WRITE_ZEROES) | BIT_ULL(VIRTIO_BLK_F_SECURE_ERASE),
};
#undef SZ
#undef OF
#endif
const unsigned long kutox_common_layout[] __attribute__((used,section(".kutox.common_layouts"))) = {
    sizeof(struct device), _Alignof(struct device), offsetof(struct device,parent),
    offsetof(struct device,driver), offsetof(struct device,bus), offsetof(struct device,groups),
    offsetof(struct device,release), offsetof(struct device,driver_data), offsetof(struct device,of_node),
    sizeof(struct pci_dev), offsetof(struct pci_dev,dev), offsetof(struct pci_dev,vendor),
    offsetof(struct pci_dev,device), offsetof(struct pci_dev,subsystem_vendor),
    offsetof(struct pci_dev,subsystem_device), offsetof(struct pci_dev,revision), offsetof(struct pci_dev,irq),
    offsetof(struct pci_dev,is_virtfn), offsetof(struct pci_dev,physfn),
    sizeof(struct pci_driver), offsetof(struct pci_driver,probe), offsetof(struct pci_driver,remove),
    offsetof(struct pci_driver,driver), offsetof(struct pci_driver,err_handler), offsetof(struct pci_driver,sriov_configure),
    sizeof(struct device_driver), sizeof(struct bus_type), offsetof(struct bus_type,probe), offsetof(struct bus_type,remove),
    sizeof(struct mutex), sizeof(struct completion), sizeof(spinlock_t), sizeof(struct work_struct),
    sizeof(struct list_head), GFP_KERNEL, GFP_ATOMIC, __GFP_ZERO,
};
#if defined(KUTOX_DRIVER_ABI_PROFILE)
#include "net/accessors.h"
#include "net/bridge.h"
#include "e1000.h"
const unsigned long kutox_packet_layout[] __attribute__((used,section(".kutox.packet_layouts"))) = {
    sizeof(struct sk_buff), _Alignof(struct sk_buff),
    offsetof(struct sk_buff,data), offsetof(struct sk_buff,len), offsetof(struct sk_buff,data_len),
    offsetof(struct sk_buff,truesize), offsetof(struct sk_buff,csum_offset),
    offsetof(struct sk_buff,ip_summed), offsetof(struct sk_buff,no_fcs), offsetof(struct sk_buff,protocol),
    offsetof(struct sk_buff,head), offsetof(struct sk_buff,tail), offsetof(struct sk_buff,end),
    offsetof(struct sk_buff,mac_header), offsetof(struct sk_buff,network_header),
    offsetof(struct sk_buff,transport_header), offsetof(struct sk_buff,device),
    offsetof(struct sk_buff,backing_kind), offsetof(struct sk_buff,vlan_present),
    offsetof(struct sk_buff,vlan_tci), offsetof(struct sk_buff,references),
    sizeof(struct page), _Alignof(struct page), offsetof(struct page,state),
    sizeof(skb_frag_t), _Alignof(skb_frag_t), offsetof(skb_frag_t,page),
    offsetof(skb_frag_t,offset), offsetof(skb_frag_t,size),
    sizeof(struct skb_shared_info), _Alignof(struct skb_shared_info),
    offsetof(struct skb_shared_info,gso_size), offsetof(struct skb_shared_info,gso_segs),
    offsetof(struct skb_shared_info,nr_frags), offsetof(struct skb_shared_info,frags),
    sizeof(struct netdev_hw_addr), _Alignof(struct netdev_hw_addr),
    offsetof(struct netdev_hw_addr,addr), offsetof(struct netdev_hw_addr,next),
    sizeof(struct net_device_stats), _Alignof(struct net_device_stats),
    offsetof(struct net_device_stats,rx_packets), offsetof(struct net_device_stats,tx_packets),
    offsetof(struct net_device_stats,rx_bytes), offsetof(struct net_device_stats,tx_bytes),
    offsetof(struct net_device_stats,rx_errors), offsetof(struct net_device_stats,tx_errors),
    offsetof(struct net_device_stats,rx_dropped), offsetof(struct net_device_stats,tx_dropped),
    offsetof(struct net_device_stats,multicast), offsetof(struct net_device_stats,collisions),
    offsetof(struct net_device_stats,rx_length_errors), offsetof(struct net_device_stats,rx_over_errors),
    offsetof(struct net_device_stats,rx_crc_errors), offsetof(struct net_device_stats,rx_frame_errors),
    offsetof(struct net_device_stats,rx_fifo_errors), offsetof(struct net_device_stats,rx_missed_errors),
    offsetof(struct net_device_stats,tx_aborted_errors), offsetof(struct net_device_stats,tx_carrier_errors),
    offsetof(struct net_device_stats,tx_fifo_errors), offsetof(struct net_device_stats,tx_heartbeat_errors),
    offsetof(struct net_device_stats,tx_window_errors),
    sizeof(struct net_device_ops), _Alignof(struct net_device_ops),
    offsetof(struct net_device_ops,ndo_open), offsetof(struct net_device_ops,ndo_stop),
    offsetof(struct net_device_ops,ndo_start_xmit), offsetof(struct net_device_ops,ndo_set_rx_mode),
    offsetof(struct net_device_ops,ndo_set_mac_address), offsetof(struct net_device_ops,ndo_tx_timeout),
    offsetof(struct net_device_ops,ndo_change_mtu), offsetof(struct net_device_ops,ndo_eth_ioctl),
    offsetof(struct net_device_ops,ndo_validate_addr), offsetof(struct net_device_ops,ndo_vlan_rx_add_vid),
    offsetof(struct net_device_ops,ndo_vlan_rx_kill_vid), offsetof(struct net_device_ops,ndo_fix_features),
    offsetof(struct net_device_ops,ndo_set_features),
    sizeof(struct net_device), _Alignof(struct net_device),
    offsetof(struct net_device,name), offsetof(struct net_device,dev_addr),
    offsetof(struct net_device,addr_len), offsetof(struct net_device,flags),
    offsetof(struct net_device,priv_flags), offsetof(struct net_device,mtu),
    offsetof(struct net_device,min_mtu), offsetof(struct net_device,max_mtu),
    offsetof(struct net_device,watchdog_timeo), offsetof(struct net_device,features),
    offsetof(struct net_device,hw_features), offsetof(struct net_device,vlan_features),
    offsetof(struct net_device,netdev_ops), offsetof(struct net_device,ethtool_ops),
    offsetof(struct net_device,stats), offsetof(struct net_device,parent),
    offsetof(struct net_device,private_data), offsetof(struct net_device,private_size),
    offsetof(struct net_device,lifecycle), offsetof(struct net_device,registered),
    offsetof(struct net_device,running), offsetof(struct net_device,carrier),
    offsetof(struct net_device,stopped), offsetof(struct net_device,detached),
    offsetof(struct net_device,sent_packets), offsetof(struct net_device,sent_bytes),
    offsetof(struct net_device,completed_packets), offsetof(struct net_device,completed_bytes),
    sizeof(struct netdev_queue), _Alignof(struct netdev_queue), offsetof(struct netdev_queue,device),
    sizeof(struct napi_struct), _Alignof(struct napi_struct), offsetof(struct napi_struct,poll),
    offsetof(struct napi_struct,generation), offsetof(struct napi_struct,device), offsetof(struct napi_struct,irq),
    SKB_DATA_ALIGN(1), MAX_SKB_FRAGS, ETH_ALEN, IFNAMSIZ, CHECKSUM_NONE, NETDEV_TX_BUSY,
};
const unsigned long kutox_accessor_layout[] __attribute__((used,section(".kutox.accessor_layouts"))) = {
    sizeof(struct kutox_e1000_view), _Alignof(struct kutox_e1000_view),
    offsetof(struct kutox_e1000_view,mmio), offsetof(struct kutox_e1000_view,tx_count),
    offsetof(struct kutox_e1000_view,rx_count), offsetof(struct kutox_e1000_view,rx_buffer_len),
    offsetof(struct kutox_e1000_view,rx_backing_size), offsetof(struct kutox_e1000_view,rx_failures),
    offsetof(struct kutox_e1000_view,clean_rx), offsetof(struct kutox_e1000_view,alloc_rx),
    offsetof(struct kutox_e1000_view,watchdog), offsetof(struct kutox_e1000_view,reset),
    offsetof(struct kutox_e1000_view,napi), offsetof(struct kutox_e1000_view,tx_use),
    offsetof(struct kutox_e1000_view,tx_clean), offsetof(struct kutox_e1000_view,rx_use),
    offsetof(struct kutox_e1000_view,rx_clean), offsetof(struct kutox_e1000_view,rx_unused),
    offsetof(struct kutox_e1000_view,tx_desc), offsetof(struct kutox_e1000_view,tx_size),
    offsetof(struct kutox_e1000_view,tx_dma), offsetof(struct kutox_e1000_view,pci_device),
    HZ, SYSTEM_RUNNING, E1000_EECD, E1000_EECD_GNT, MAXIMUM_ETHERNET_VLAN_SIZE,
    E1000_RCTL, E1000_TCTL, E1000_RCTL_EN, E1000_TCTL_EN,
    sizeof(struct iphdr), sizeof(struct ipv6hdr), sizeof(struct tcphdr),
    sizeof(struct kutox_result), _Alignof(struct kutox_result), offsetof(struct kutox_result,code),
    offsetof(struct kutox_result,stage), offsetof(struct kutox_result,denied_grants),
    offsetof(struct kutox_result,eeprom_reads), offsetof(struct kutox_result,mac),
};
#endif
