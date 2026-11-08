/* SPDX-License-Identifier: GPL-2.0-only */
#include "driver_services.h"
static int unsupported(const char *operation)
{
    kutox_driver_diag(operation,-EOPNOTSUPP); return -EOPNOTSUPP;
}
enum system_states system_state = SYSTEM_RUNNING;
int pcix_get_mmrbc(struct pci_dev *device) { return unsupported("pcix_get"); }
int pcix_set_mmrbc(struct pci_dev *device, int value) { return unsupported("pcix_set"); }
void pci_enable_wake(struct pci_dev *device, pci_power_t state, bool enable) { kutox_platform_fail("pci_wake_unsupported"); }
void pci_wake_from_d3(struct pci_dev *device, bool enable) { kutox_platform_fail("pci_wake_d3_unsupported"); }
int pci_set_power_state(struct pci_dev *device, pci_power_t state) { return unsupported("pci_power"); }
dma_addr_t dma_map_page(struct device *device, struct page *page, size_t offset, size_t size, enum dma_data_direction direction)
{
    kutox_driver_diag("page_dma_profile",-EOPNOTSUPP); return UINT64_MAX;
}
void dma_unmap_page(struct device *device, dma_addr_t address, size_t size, enum dma_data_direction direction) { kutox_platform_fail("page_unmap_profile"); }
dma_addr_t skb_frag_dma_map(struct device *device, const skb_frag_t *fragment, size_t offset, size_t size, enum dma_data_direction direction)
{
    return dma_map_page(device, fragment->page, offset, size, direction);
}
struct page *alloc_page(gfp_t flags) { unsupported("alloc_page_profile"); return NULL; }
void put_page(struct page *page) { kutox_platform_fail("put_page_profile"); }
void *page_address(const struct page *page) { kutox_platform_fail("page_address_profile"); }
unsigned long page_to_phys(const struct page *page) { kutox_platform_fail("page_phys_profile"); }
struct page *skb_frag_page(const skb_frag_t *fragment) { if (!fragment) kutox_platform_fail("fragment_null"); return fragment->page; }
unsigned int skb_frag_size(const skb_frag_t *fragment) { if (!fragment) kutox_platform_fail("fragment_null"); return fragment->size; }
void skb_fill_page_desc(struct sk_buff *skb, int index, struct page *page, int offset, int size) { kutox_platform_fail("fragment_packet_profile"); }
struct sk_buff *napi_get_frags(struct napi_struct *napi) { kutox_platform_fail("jumbo_napi_profile"); }
int napi_gro_frags(struct napi_struct *napi) { return unsupported("jumbo_ingress"); }
int pskb_trim(struct sk_buff *skb, unsigned int length)
{
    if (skb->data_len) return unsupported("fragment_trim");
    skb_trim(skb,length); return 0;
}
void *__pskb_pull_tail(struct sk_buff *skb, int length) { kutox_platform_fail("fragment_pull_tail"); }
void __vlan_hwaccel_put_tag(struct sk_buff *skb, __be16 protocol, u16 tag) { kutox_platform_fail("vlan_profile"); }
__sum16 csum_tcpudp_magic(__be32 a, __be32 b, unsigned int length, unsigned short protocol, __wsum sum) { kutox_platform_fail("checksum_offload_profile"); }
static void *header(const struct sk_buff *skb, unsigned int offset, size_t size)
{
    if (!skb || offset > (size_t)(skb->tail - skb->head) || size > (size_t)(skb->tail - skb->head) - offset)
        kutox_platform_fail("skb_header_bounds");
    driver_check(kutox_driver_range((uintptr_t)(skb->head + offset),size,0), "skb_header_device_owned");
    return skb->head + offset;
}
struct iphdr *ip_hdr(const struct sk_buff *skb) { return header(skb,skb->network_header,sizeof(struct iphdr)); }
struct ipv6hdr *ipv6_hdr(const struct sk_buff *skb) { return header(skb,skb->network_header,sizeof(struct ipv6hdr)); }
struct tcphdr *tcp_hdr(const struct sk_buff *skb) { return header(skb,skb->transport_header,sizeof(struct tcphdr)); }
unsigned int skb_network_offset(const struct sk_buff *skb) { return skb->network_header - (unsigned int)(skb->data - skb->head); }
unsigned int skb_transport_offset(const struct sk_buff *skb) { return skb->transport_header - (unsigned int)(skb->data - skb->head); }
unsigned int skb_tcp_all_headers(const struct sk_buff *skb) { return skb_transport_offset(skb) + tcp_hdr(skb)->doff * 4U; }
u16 skb_checksum_start_offset(const struct sk_buff *skb) { return (u16)skb_transport_offset(skb); }
bool skb_is_gso_v6(const struct sk_buff *skb) { return skb_is_gso(skb) && skb->protocol == htons(ETH_P_IPV6); }
void tcp_v6_gso_csum_prep(struct sk_buff *skb) { kutox_platform_fail("gso_profile"); }
int skb_cow_head(struct sk_buff *skb, unsigned int headroom) { return unsupported("skb_cow_profile"); }
int ethtool_op_get_ts_info(struct net_device *device, struct kernel_ethtool_ts_info *info) { return unsupported("ethtool_timestamp_profile"); }
void ethtool_convert_legacy_u32_to_link_mode(unsigned long *bitmap, u32 legacy)
{
    if (!bitmap) kutox_platform_fail("ethtool_bitmap_null");
    memset(bitmap,0,4 * sizeof(*bitmap)); bitmap[0] = legacy;
}
bool ethtool_convert_link_mode_to_legacy_u32(u32 *legacy, const unsigned long *bitmap)
{
    if (!legacy || !bitmap) kutox_platform_fail("ethtool_bitmap_null");
    if (bitmap[0] > UINT32_MAX || bitmap[1] || bitmap[2] || bitmap[3]) return false;
    *legacy = (u32)bitmap[0]; return true;
}
struct mii_ioctl_data *if_mii(struct ifreq *request) { kutox_platform_fail("mii_ioctl_profile"); }
