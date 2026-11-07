/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef KUTOX_DRIVER_CHECK_PACKET_H
#define KUTOX_DRIVER_CHECK_PACKET_H
typedef u64 netdev_features_t;
#include <linux/netdev_features.h>
#define NET_SKB_PAD 64
typedef enum { NETDEV_TX_OK, NETDEV_TX_BUSY = 0x10 } netdev_tx_t;
struct sk_buff {
    unsigned char *data;
    unsigned int len, data_len, truesize;
    u16 csum_offset;
    u8 ip_summed, no_fcs;
    __be16 protocol;
    unsigned char *head, *tail, *end;
    unsigned int mac_header, network_header, transport_header;
    struct net_device *device;
    u8 backing_kind, vlan_present;
    u16 vlan_tci;
    unsigned int references;
};
struct page { unsigned long state; };
typedef struct { struct page *page; unsigned int offset, size; } skb_frag_t;
struct skb_shared_info { u16 gso_size, gso_segs; unsigned int nr_frags; skb_frag_t frags[17]; };
struct vlan_hdr { __be16 h_vlan_TCI, h_vlan_encapsulated_proto; };
struct netdev_hw_addr { unsigned char addr[ETH_ALEN]; struct netdev_hw_addr *next; };
struct net_device_stats {
    unsigned long rx_packets, tx_packets, rx_bytes, tx_bytes, rx_errors, tx_errors;
    unsigned long rx_dropped, tx_dropped, multicast, collisions, rx_length_errors;
    unsigned long rx_over_errors, rx_crc_errors, rx_frame_errors, rx_fifo_errors;
    unsigned long rx_missed_errors, tx_aborted_errors, tx_carrier_errors;
    unsigned long tx_fifo_errors, tx_heartbeat_errors, tx_window_errors;
};
struct ethtool_ops;
struct net_device_ops {
    int (*ndo_open)(struct net_device *);
    int (*ndo_stop)(struct net_device *);
    netdev_tx_t (*ndo_start_xmit)(struct sk_buff *, struct net_device *);
    void (*ndo_set_rx_mode)(struct net_device *);
    int (*ndo_set_mac_address)(struct net_device *, void *);
    void (*ndo_tx_timeout)(struct net_device *, unsigned int);
    int (*ndo_change_mtu)(struct net_device *, int);
    int (*ndo_eth_ioctl)(struct net_device *, struct ifreq *, int);
    int (*ndo_validate_addr)(struct net_device *);
    int (*ndo_vlan_rx_add_vid)(struct net_device *, __be16, u16);
    int (*ndo_vlan_rx_kill_vid)(struct net_device *, __be16, u16);
    netdev_features_t (*ndo_fix_features)(struct net_device *, netdev_features_t);
    int (*ndo_set_features)(struct net_device *, netdev_features_t);
};
struct net_device {
    char name[IFNAMSIZ];
    unsigned char dev_addr[ETH_ALEN];
    unsigned int addr_len, flags, priv_flags;
    int mtu, min_mtu, max_mtu;
    unsigned long watchdog_timeo;
    netdev_features_t features, hw_features, vlan_features;
    const struct net_device_ops *netdev_ops;
    const struct ethtool_ops *ethtool_ops;
    struct net_device_stats stats;
    struct device *parent;
    void *private_data;
    size_t private_size;
    unsigned int lifecycle;
    bool registered, running, carrier, stopped, detached;
    unsigned long sent_packets, sent_bytes, completed_packets, completed_bytes;
};
#define IFF_SUPP_NOFCS BIT(14)
#define IFF_UNICAST_FLT BIT(12)
#define NETIF_MSG_DRV BIT(0)
#define NETIF_MSG_PROBE BIT(1)
#define NETIF_MSG_LINK BIT(2)
#define NETDEV_QUEUE_TYPE_RX 0
#define NETDEV_QUEUE_TYPE_TX 1
#define CHECKSUM_NONE 0
#define CHECKSUM_UNNECESSARY 1
#define CHECKSUM_PARTIAL 3
#define MAX_SKB_FRAGS 17
#define VLAN_N_VID 4096
#define VLAN_VID_MASK 0x0fff
#define SKB_DATA_ALIGN(n) ALIGN(n,64)
struct net_device *alloc_etherdev(size_t);
void free_netdev(struct net_device *);
int register_netdev(struct net_device *);
void unregister_netdev(struct net_device *);
void *netdev_priv(const struct net_device *);
void set_netdev_dev(struct net_device *, struct device *);
#define SET_NETDEV_DEV(n,d) set_netdev_dev(n,d)
void netif_carrier_off(struct net_device *);
void netif_carrier_on(struct net_device *);
bool netif_carrier_ok(const struct net_device *);
bool netif_running(const struct net_device *);
bool netif_queue_stopped(const struct net_device *);
void netif_start_queue(struct net_device *);
void netif_stop_queue(struct net_device *);
void netif_wake_queue(struct net_device *);
void netif_tx_disable(struct net_device *);
void netif_device_detach(struct net_device *);
void netif_device_attach(struct net_device *);
void netif_napi_add(struct net_device *, struct napi_struct *, int (*)(struct napi_struct *,int));
void netif_napi_set_irq(struct napi_struct *, unsigned int);
void netif_queue_set_napi(struct net_device *, unsigned int, unsigned int, struct napi_struct *);
void netdev_sent_queue(struct net_device *, unsigned int);
void netdev_completed_queue(struct net_device *, unsigned int, unsigned int);
void netdev_reset_queue(struct net_device *);
void netif_trans_update(struct net_device *);
u32 netif_msg_init(int, int);
bool netif_msg_tx_done(u32);
bool netif_msg_rx_status(u32);
bool netif_msg_pktdata(const void *);
bool netif_msg_tx_queued(const void *);
bool netif_msg_intr(const void *);
bool netif_msg_hw(u32);
#define netif_msg_hw(a) netif_msg_hw((a)->msg_enable)
#define netif_msg_rx_status(a) netif_msg_rx_status((a)->msg_enable)
#define netif_msg_tx_done(a) netif_msg_tx_done((a)->msg_enable)
void eth_hw_addr_set(struct net_device *, const u8 *);
bool is_valid_ether_addr(const u8 *);
bool is_multicast_ether_addr(const u8 *);
bool is_zero_ether_addr(const u8 *);
bool is_broadcast_ether_addr(const u8 *);
int eth_validate_addr(struct net_device *);
void ether_addr_copy(u8 *, const u8 *);
void eth_zero_addr(u8 *);
unsigned int netdev_uc_count(const struct net_device *);
unsigned int netdev_mc_count(const struct net_device *);
bool netdev_uc_empty(const struct net_device *);
struct netdev_hw_addr *netdev_mc_first(const struct net_device *);
struct netdev_hw_addr *netdev_uc_first(const struct net_device *);
struct netdev_hw_addr *netdev_addr_next(struct netdev_hw_addr *);
#define netdev_for_each_mc_addr(a,d) for ((a)=netdev_mc_first(d); (a); (a)=netdev_addr_next(a))
#define netdev_for_each_uc_addr(a,d) for ((a)=netdev_uc_first(d); (a); (a)=netdev_addr_next(a))
struct skb_shared_info *skb_shinfo(struct sk_buff *);
unsigned int skb_headlen(const struct sk_buff *);
bool skb_is_gso(const struct sk_buff *);
bool skb_header_cloned(const struct sk_buff *);
struct sk_buff *netdev_alloc_skb(struct net_device *, unsigned int);
struct sk_buff *netdev_alloc_skb_ip_align(struct net_device *, unsigned int);
struct sk_buff *alloc_skb(unsigned int, gfp_t);
struct sk_buff *napi_build_skb(void *, unsigned int);
void *netdev_alloc_frag(unsigned int);
void skb_free_frag(void *);
void skb_reserve(struct sk_buff *, unsigned int);
void *skb_put(struct sk_buff *, unsigned int);
void skb_put_data(struct sk_buff *, const void *, unsigned int);
void skb_trim(struct sk_buff *, unsigned int);
void *skb_pull(struct sk_buff *, unsigned int);
void *skb_tail_pointer(const struct sk_buff *);
unsigned int skb_tailroom(const struct sk_buff *);
int eth_skb_pad(struct sk_buff *);
int skb_pad(struct sk_buff *, int);
void dev_kfree_skb_any(struct sk_buff *);
void dev_kfree_skb_irq(struct sk_buff *);
void dev_consume_skb_any(struct sk_buff *);
void napi_consume_skb(struct sk_buff *, int);
void consume_skb(struct sk_buff *);
void kfree_skb(struct sk_buff *);
void *skb_mac_header(const struct sk_buff *);
struct ethhdr *eth_hdr(const struct sk_buff *);
void skb_set_network_header(struct sk_buff *, int);
void skb_set_transport_header(struct sk_buff *, int);
void skb_reset_mac_header(struct sk_buff *);
struct iphdr *ip_hdr(const struct sk_buff *);
struct ipv6hdr *ipv6_hdr(const struct sk_buff *);
struct tcphdr *tcp_hdr(const struct sk_buff *);
unsigned int skb_tcp_all_headers(const struct sk_buff *);
unsigned int skb_transport_offset(const struct sk_buff *);
unsigned int skb_network_offset(const struct sk_buff *);
bool skb_is_gso_v6(const struct sk_buff *);
void tcp_v6_gso_csum_prep(struct sk_buff *);
void skb_tx_timestamp(struct sk_buff *);
void skb_checksum_none_assert(const struct sk_buff *);
bool netdev_xmit_more(void);
struct netdev_queue;
struct netdev_queue *netdev_get_tx_queue(const struct net_device *, unsigned int);
bool netif_xmit_stopped(const struct netdev_queue *);
void *__pskb_pull_tail(struct sk_buff *, int);
int pskb_expand_head(struct sk_buff *, int, int, gfp_t);
int skb_cow_head(struct sk_buff *, unsigned int);
u16 skb_checksum_start_offset(const struct sk_buff *);
bool skb_vlan_tag_present(const struct sk_buff *);
u16 skb_vlan_tag_get(const struct sk_buff *);
void __vlan_hwaccel_put_tag(struct sk_buff *, __be16, u16);
__be16 vlan_get_protocol(const struct sk_buff *);
__be16 eth_type_trans(struct sk_buff *, struct net_device *);
int napi_gro_receive(struct napi_struct *, struct sk_buff *);
__sum16 csum_tcpudp_magic(__be32,__be32,unsigned int,unsigned short,__wsum);
__sum16 csum_ipv6_magic(const struct in6_addr *, const struct in6_addr *, u32,u8,__wsum);
unsigned int skb_frag_size(const skb_frag_t *);
unsigned int skb_frag_off(const skb_frag_t *);
struct page *skb_frag_page(const skb_frag_t *);
dma_addr_t skb_frag_dma_map(struct device *, const skb_frag_t *, size_t,size_t,enum dma_data_direction);
unsigned long page_to_phys(const struct page *);
struct page *alloc_page(gfp_t);
void put_page(struct page *);
void get_page(struct page *);
void *page_address(const struct page *);
void skb_add_rx_frag(struct sk_buff *, int, struct page *, int,int,unsigned int);
void skb_fill_page_desc(struct sk_buff *, int, struct page *, int,int);
struct mii_ioctl_data *if_mii(struct ifreq *);
bool capable(int);
struct sk_buff *napi_alloc_skb(struct napi_struct *, unsigned int);
struct sk_buff *napi_get_frags(struct napi_struct *);
void napi_free_frags(struct napi_struct *);
int napi_gro_frags(struct napi_struct *);
int pskb_trim(struct sk_buff *, unsigned int);
void dev_kfree_skb(struct sk_buff *);
#endif
