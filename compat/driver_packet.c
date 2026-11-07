/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver_services.h"

static struct net_device *netdev;
struct netdev_queue { struct net_device *device; };
static struct netdev_queue tx_queue;
static void *fragments[512];
extern int kutox_driver_ingress(uintptr_t, size_t);
extern void kutox_c_retire_napi_device(struct net_device *);
extern void kutox_c_pci_clear_data(void *);

static void netdev_check(const struct net_device *device)
{
    if (!device || device != netdev) kutox_platform_fail("netdev_identity");
}
struct net_device *alloc_etherdev(size_t private_size)
{
    if (netdev || !private_size || private_size > 65536) {
        kutox_driver_diag("netdev_allocation_profile", -EINVAL); return NULL;
    }
    struct net_device *device = kzalloc(sizeof(*device), GFP_KERNEL);
    if (!device) return NULL;
    void *private_data = kzalloc(private_size, GFP_KERNEL);
    if (!private_data) { kfree(device); return NULL; }
    device->private_data = private_data; device->private_size = private_size;
    device->mtu = 1500; device->addr_len = ETH_ALEN;
    device->stopped = true; device->detached = false;
    strcpy(device->name, "eth0");
    netdev = device;
    return device;
}
void free_netdev(struct net_device *device)
{
    netdev_check(device);
    if (device->registered || device->running || device->sent_packets != device->completed_packets)
        kutox_platform_fail("free_netdev_live");
    kutox_c_retire_napi_device(device);
    kutox_c_pci_clear_data(device);
    kfree(device->private_data);
    kfree(device); netdev = NULL; tx_queue.device = NULL;
}
void *netdev_priv(const struct net_device *device) { netdev_check(device); return device->private_data; }
void set_netdev_dev(struct net_device *device, struct device *parent)
{
    netdev_check(device);
    if ((uintptr_t)parent != kutox_driver_device()) kutox_platform_fail("netdev_parent");
    device->parent = parent;
}
int register_netdev(struct net_device *device)
{
    netdev_check(device);
    if (device->registered || !device->parent || !device->netdev_ops || !device->netdev_ops->ndo_open ||
        !device->netdev_ops->ndo_stop || !device->netdev_ops->ndo_start_xmit) return -EINVAL;
    if (device->mtu != 1500 || eth_validate_addr(device)) return -EINVAL;
    /* Initial profile negotiates offloads off through normal feature operations. */
    netdev_features_t selected = 0;
    if (device->netdev_ops->ndo_fix_features) selected = device->netdev_ops->ndo_fix_features(device, selected);
    if (selected) return -EOPNOTSUPP;
    if (device->netdev_ops->ndo_set_features) {
        int result = device->netdev_ops->ndo_set_features(device, selected);
        if (result < 0) return result;
    }
    device->features = selected; device->vlan_features = 0;
    device->registered = true; strcpy(device->name, "eth0");
    return 0;
}
int kutox_c_netdev_open(struct net_device *device)
{
    netdev_check(device);
    if (!device->registered || device->running || device->detached) return -EINVAL;
    device->running = true;
    int result = device->netdev_ops->ndo_open(device);
    if (result < 0) { device->running = false; kutox_driver_gate(); }
    return result;
}
void unregister_netdev(struct net_device *device)
{
    netdev_check(device);
    if (!device->registered) kutox_platform_fail("netdev_unregister_unregistered");
    if (device->running) {
        device->running = false;
        driver_check(device->netdev_ops->ndo_stop(device), "netdev_stop");
    }
    device->registered = false;
}
void netif_carrier_off(struct net_device *device) { netdev_check(device); device->carrier = false; }
void netif_carrier_on(struct net_device *device) { netdev_check(device); device->carrier = true; }
bool netif_carrier_ok(const struct net_device *device) { netdev_check(device); return device->carrier; }
bool netif_running(const struct net_device *device) { netdev_check(device); return device->running; }
void netif_device_detach(struct net_device *device) { netdev_check(device); device->detached = true; device->stopped = true; }
void netif_device_attach(struct net_device *device) { netdev_check(device); device->detached = false; }
void netif_start_queue(struct net_device *device) { netdev_check(device); device->stopped = false; }
void netif_stop_queue(struct net_device *device) { netdev_check(device); device->stopped = true; }
void netif_wake_queue(struct net_device *device) { netdev_check(device); if (!device->detached) device->stopped = false; }
void netif_tx_disable(struct net_device *device) { netif_stop_queue(device); }
bool netif_queue_stopped(const struct net_device *device) { netdev_check(device); return device->stopped || device->detached; }
void netdev_sent_queue(struct net_device *device, unsigned int bytes)
{
    netdev_check(device);
    if (!bytes || device->sent_packets == ~0UL || device->sent_bytes > ~0UL - bytes)
        kutox_platform_fail("netdev_sent_overflow");
    device->sent_packets++; device->sent_bytes += bytes;
}
void netdev_completed_queue(struct net_device *device, unsigned int packets, unsigned int bytes)
{
    netdev_check(device);
    if (packets > device->sent_packets - device->completed_packets ||
        bytes > device->sent_bytes - device->completed_bytes) kutox_platform_fail("netdev_completion_accounting");
    device->completed_packets += packets; device->completed_bytes += bytes;
}
void netdev_reset_queue(struct net_device *device)
{
    netdev_check(device);
    if (!device->stopped) kutox_platform_fail("reset_running_queue");
    device->sent_packets = device->completed_packets = device->sent_bytes = device->completed_bytes = 0;
}
struct netdev_queue *netdev_get_tx_queue(const struct net_device *device, unsigned int index)
{
    netdev_check(device); if (index) kutox_platform_fail("netdev_queue_index");
    tx_queue.device = netdev; return &tx_queue;
}
bool netif_xmit_stopped(const struct netdev_queue *queue)
{
    if (queue != &tx_queue) kutox_platform_fail("netdev_queue_identity");
    return netif_queue_stopped(queue->device);
}
bool netdev_xmit_more(void) { return false; /* Native adapter submits one packet per call. */ }
void netif_trans_update(struct net_device *device) { netdev_check(device); device->lifecycle++; }
unsigned int netdev_uc_count(const struct net_device *device) { netdev_check(device); return 0; }
unsigned int netdev_mc_count(const struct net_device *device) { netdev_check(device); return 0; }
struct netdev_hw_addr *netdev_uc_first(const struct net_device *device) { netdev_check(device); return NULL; }
struct netdev_hw_addr *netdev_mc_first(const struct net_device *device) { netdev_check(device); return NULL; }
struct netdev_hw_addr *netdev_addr_next(struct netdev_hw_addr *address)
{
    if (!address) kutox_platform_fail("address_iterator_null");
    return address->next;
}
bool is_multicast_ether_addr(const u8 *address) { if (!address) kutox_platform_fail("ethernet_null"); return address[0] & 1; }
bool is_broadcast_ether_addr(const u8 *address)
{
    if (!address) kutox_platform_fail("ethernet_null");
    for (unsigned int index = 0; index < ETH_ALEN; index++) if (address[index] != 255) return false;
    return true;
}
bool is_valid_ether_addr(const u8 *address)
{
    if (is_multicast_ether_addr(address)) return false;
    u8 bits = 0; for (unsigned int index = 0; index < ETH_ALEN; index++) bits |= address[index];
    return bits != 0;
}
void eth_hw_addr_set(struct net_device *device, const u8 *address)
{
    netdev_check(device); if (!address) kutox_platform_fail("ethernet_null");
    memcpy(device->dev_addr, address, ETH_ALEN);
}
int eth_validate_addr(struct net_device *device) { netdev_check(device); return is_valid_ether_addr(device->dev_addr) ? 0 : -EADDRNOTAVAIL; }

static void skb_check(const struct sk_buff *skb)
{
    if (!skb || kutox_driver_size((uintptr_t)skb) != sizeof(*skb) || !skb->references ||
        skb->data < skb->head || skb->tail < skb->data || skb->end < skb->tail ||
        skb->data_len || skb->len != (unsigned int)(skb->tail - skb->data))
        kutox_platform_fail("skb_metadata");
    size_t backing = kutox_driver_size((uintptr_t)skb->head);
    if (backing < sizeof(struct skb_shared_info) ||
        (size_t)(skb->end - skb->head) > backing - sizeof(struct skb_shared_info))
        kutox_platform_fail("skb_backing_bounds");
}
struct sk_buff *alloc_skb(unsigned int size, gfp_t flags)
{
    if (size > 4096 || !size) { kutox_driver_diag("skb_profile", -EINVAL); return NULL; }
    struct sk_buff *skb = kzalloc(sizeof(*skb), flags);
    if (!skb) return NULL;
    size_t capacity = SKB_DATA_ALIGN(size) + SKB_DATA_ALIGN(sizeof(struct skb_shared_info));
    u8 *head = kzalloc(capacity, flags);
    if (!head) { kfree(skb); return NULL; }
    skb->head = skb->data = skb->tail = head;
    skb->end = head + SKB_DATA_ALIGN(size);
    skb->truesize = (unsigned int)(capacity + sizeof(*skb));
    skb->references = 1; skb->backing_kind = 0;
    return skb;
}
struct skb_shared_info *skb_shinfo(struct sk_buff *skb)
{
    skb_check(skb);
    return (struct skb_shared_info *)skb->end;
}
bool skb_is_gso(const struct sk_buff *skb) { return skb_shinfo((struct sk_buff *)skb)->gso_size != 0; }
unsigned int skb_headlen(const struct sk_buff *skb) { skb_check(skb); return skb->len; }
void skb_reserve(struct sk_buff *skb, unsigned int size)
{
    skb_check(skb);
    if (skb->len || size > (size_t)(skb->end - skb->tail)) kutox_platform_fail("skb_reserve");
    skb->data += size; skb->tail += size;
}
void *skb_put(struct sk_buff *skb, unsigned int size)
{
    skb_check(skb);
    if (size > (size_t)(skb->end - skb->tail)) kutox_platform_fail("skb_put");
    u8 *tail = skb->tail;
    if (size) driver_check(kutox_driver_range((uintptr_t)tail,size,0), "skb_put_device_owned");
    skb->tail += size; skb->len += size;
    return tail;
}
void skb_put_data(struct sk_buff *skb, const void *data, unsigned int size)
{
    if (!data && size) kutox_platform_fail("skb_copy_null");
    memcpy(skb_put(skb,size), data, size);
}
void *skb_tail_pointer(const struct sk_buff *skb) { skb_check(skb); return skb->tail; }
unsigned int skb_tailroom(const struct sk_buff *skb) { skb_check(skb); return (unsigned int)(skb->end - skb->tail); }
void skb_trim(struct sk_buff *skb, unsigned int size)
{
    skb_check(skb); if (size > skb->len) return;
    skb->tail = skb->data + size; skb->len = size;
}
void *skb_pull(struct sk_buff *skb, unsigned int size)
{
    skb_check(skb); if (size > skb->len) return NULL;
    skb->data += size; skb->len -= size; return skb->data;
}
static void free_skb(struct sk_buff *skb)
{
    skb_check(skb);
    if (skb->references != 1) kutox_platform_fail("skb_reference_profile");
    if (skb->backing_kind) skb_free_frag(skb->head); else kfree(skb->head);
    skb->references = 0; kfree(skb);
}
void dev_kfree_skb(struct sk_buff *skb) { if (skb) free_skb(skb); }
void dev_kfree_skb_any(struct sk_buff *skb) { dev_kfree_skb(skb); }
void napi_consume_skb(struct sk_buff *skb, int budget)
{
    if (budget < 0 || budget > 64) kutox_platform_fail("skb_consume_budget");
    dev_kfree_skb(skb);
}
int eth_skb_pad(struct sk_buff *skb)
{
    skb_check(skb); if (skb->len >= 60) return 0;
    unsigned int padding = 60 - skb->len;
    if (skb_tailroom(skb) >= padding) { memset(skb_put(skb,padding),0,padding); return 0; }
    size_t headroom = (size_t)(skb->data - skb->head);
    size_t capacity = SKB_DATA_ALIGN(headroom + 60) + SKB_DATA_ALIGN(sizeof(struct skb_shared_info));
    u8 *head = kmalloc(capacity,GFP_ATOMIC);
    if (!head) { free_skb(skb); return -ENOMEM; }
    driver_check(kutox_driver_range((uintptr_t)skb->data,skb->len,0), "pad_device_owned");
    memcpy(head + headroom, skb->data, skb->len);
    if (skb->backing_kind) skb_free_frag(skb->head); else kfree(skb->head);
    skb->head = head; skb->data = head + headroom; skb->tail = skb->data + skb->len;
    skb->end = head + SKB_DATA_ALIGN(headroom + 60); skb->backing_kind = 0;
    skb->truesize = (unsigned int)(capacity + sizeof(*skb));
    memset(skb_put(skb,padding),0,padding);
    return 0;
}
void skb_tx_timestamp(struct sk_buff *skb) { skb_check(skb); }
void skb_checksum_none_assert(const struct sk_buff *skb) { skb_check(skb); if (skb->ip_summed != CHECKSUM_NONE) kutox_platform_fail("skb_checksum_profile"); }

int kutox_c_netdev_transmit(struct net_device *device, struct sk_buff *skb)
{
    netdev_check(device);
    if (!device->registered || !device->running || !device->carrier || device->detached || device->stopped)
        return NETDEV_TX_BUSY;
    skb_check(skb);
    struct skb_shared_info *info = skb_shinfo(skb);
    driver_check(kutox_driver_range((uintptr_t)skb->data,skb->len,0), "transmit_device_owned");
    if (skb->len < ETH_HLEN || skb->len > 1514 || info->gso_size || info->nr_frags ||
        skb->ip_summed != CHECKSUM_NONE || skb->vlan_present ||
        (skb->data[12] == 0x81 && skb->data[13] == 0) ||
        (skb->data[12] == 0x88 && skb->data[13] == 0xa8)) {
        kutox_driver_diag("transmit_profile",-EOPNOTSUPP);
        return -EOPNOTSUPP;
    }
    int result = device->netdev_ops->ndo_start_xmit(skb,device);
    if (result != NETDEV_TX_OK && result != NETDEV_TX_BUSY) kutox_platform_fail("transmit_return");
    return result;
}

bool skb_vlan_tag_present(const struct sk_buff *skb) { skb_check(skb); return skb->vlan_present; }
u16 skb_vlan_tag_get(const struct sk_buff *skb) { skb_check(skb); if (!skb->vlan_present) kutox_platform_fail("skb_vlan_absent"); return skb->vlan_tci; }
__be16 vlan_get_protocol(const struct sk_buff *skb) { skb_check(skb); return skb->protocol; }

void *netdev_alloc_frag(unsigned int size)
{
    if (!size || size > 4096) { kutox_driver_diag("fragment_profile", -EINVAL); return NULL; }
    size_t slot;
    for (slot = 0; slot < ARRAY_SIZE(fragments); slot++) if (!fragments[slot]) break;
    if (slot == ARRAY_SIZE(fragments)) { kutox_driver_diag("fragment_capacity", -ENOMEM); return NULL; }
    void *pointer = (void *)kutox_driver_alloc(size,GFP_ATOMIC,1);
    if (pointer) fragments[slot] = pointer;
    return pointer;
}
static size_t fragment_slot(const void *pointer)
{
    for (size_t slot = 0; slot < ARRAY_SIZE(fragments); slot++) {
        uintptr_t base = (uintptr_t)fragments[slot];
        if (base && (uintptr_t)pointer >= base && (uintptr_t)pointer - base < kutox_driver_size(base)) return slot;
    }
    kutox_platform_fail("fragment_provenance");
}
void skb_free_frag(void *pointer)
{
    size_t slot = fragment_slot(pointer);
    driver_check(kutox_driver_free((uintptr_t)fragments[slot]), "fragment_free_pinned");
    fragments[slot] = NULL;
}
struct sk_buff *napi_build_skb(void *pointer, unsigned int size)
{
    size_t slot = fragment_slot(pointer);
    if (fragments[slot] != pointer || kutox_driver_size((uintptr_t)pointer) != size ||
        size < SKB_DATA_ALIGN(sizeof(struct skb_shared_info))) kutox_platform_fail("build_skb_provenance");
    struct sk_buff *skb = kzalloc(sizeof(*skb),GFP_ATOMIC);
    if (!skb) return NULL;
    driver_check(kutox_driver_range((uintptr_t)pointer,size,1), "build_skb_claim");
    skb->head = skb->data = skb->tail = pointer;
    skb->end = (u8 *)pointer + size - SKB_DATA_ALIGN(sizeof(struct skb_shared_info));
    driver_check(kutox_driver_range((uintptr_t)skb->end,sizeof(struct skb_shared_info),0), "build_skb_sharedinfo_owned");
    memset(skb->end,0,sizeof(struct skb_shared_info));
    skb->references = 1; skb->backing_kind = 1; skb->truesize = size + sizeof(*skb);
    return skb;
}
struct sk_buff *napi_alloc_skb(struct napi_struct *napi, unsigned int size)
{
    if (!napi || napi->device != netdev) kutox_platform_fail("napi_skb_identity");
    struct sk_buff *skb = alloc_skb(size + 66,GFP_ATOMIC);
    if (skb) skb_reserve(skb,66);
    return skb;
}
void napi_free_frags(struct napi_struct *napi)
{
    if (!napi || napi->device != netdev) kutox_platform_fail("napi_frags_identity");
    /* Non-jumbo profile never acquires a cached fragment skb. */
}
__be16 eth_type_trans(struct sk_buff *skb, struct net_device *device)
{
    netdev_check(device); skb_check(skb);
    if (skb->len < ETH_HLEN) kutox_platform_fail("ethernet_short");
    driver_check(kutox_driver_range((uintptr_t)skb->data,skb->len,0), "ethernet_device_owned");
    skb->mac_header = (unsigned int)(skb->data - skb->head);
    skb->device = device;
    u16 type = ((u16)skb->data[12] << 8) | skb->data[13];
    skb_pull(skb,ETH_HLEN);
    if (type < 1536) kutox_platform_fail("ethernet_llc_profile");
    skb->protocol = htons(type); return skb->protocol;
}
int napi_gro_receive(struct napi_struct *napi, struct sk_buff *skb)
{
    if (!napi || napi->device != netdev || skb->device != netdev) kutox_platform_fail("ingress_identity");
    skb_check(skb);
    u8 *frame = skb->head + skb->mac_header;
    size_t size = (size_t)(skb->tail - frame);
    if (size < 14 || size > 1514) kutox_platform_fail("ingress_mtu_profile");
    driver_check(kutox_driver_range((uintptr_t)frame,size,0), "ingress_device_owned");
    int result = kutox_driver_ingress((uintptr_t)frame,size);
    free_skb(skb);
    return result;
}
bool kutox_c_packet_empty(void)
{
    for (size_t slot = 0; slot < ARRAY_SIZE(fragments); slot++) if (fragments[slot]) return false;
    return !netdev && !tx_queue.device;
}
