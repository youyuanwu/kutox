// SPDX-License-Identifier: GPL-2.0-only
use super::*;
use core::ptr;
#[unsafe(no_mangle)]
pub static mut system_state: i32 = 3;
unsafe extern "C" {
    fn kutox_tcp_header_bytes(header: *const c_void) -> u32;
}
#[unsafe(no_mangle)]
pub extern "C" fn pcix_get_mmrbc(_d: *mut PciDevice) -> i32 {
    diagnostic(c"pcix_get", -95)
}
#[unsafe(no_mangle)]
pub extern "C" fn pcix_set_mmrbc(_d: *mut PciDevice, _v: i32) -> i32 {
    diagnostic(c"pcix_set", -95)
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_enable_wake(_d: *mut PciDevice, _state: i32, _enable: bool) {
    require(false, c"pci_wake_unsupported");
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_wake_from_d3(_d: *mut PciDevice, _enable: bool) {
    require(false, c"pci_wake_d3_unsupported");
}
#[unsafe(no_mangle)]
pub extern "C" fn pci_set_power_state(_d: *mut PciDevice, _state: i32) -> i32 {
    diagnostic(c"pci_power", -95)
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_map_page(
    _d: *mut Device,
    _page: *mut Page,
    _offset: usize,
    _size: usize,
    _direction: u32,
) -> u64 {
    diagnostic(c"page_dma_profile", -95);
    u64::MAX
}
#[unsafe(no_mangle)]
pub extern "C" fn dma_unmap_page(_d: *mut Device, _address: u64, _size: usize, _direction: u32) {
    require(false, c"page_unmap_profile");
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_frag_dma_map(
    d: *mut Device,
    fragment: *const SkbFrag,
    offset: usize,
    size: usize,
    direction: u32,
) -> u64 {
    require(!fragment.is_null(), c"fragment_null");
    unsafe { dma_map_page(d, (*fragment).page, offset, size, direction) }
}
#[unsafe(no_mangle)]
pub extern "C" fn alloc_page(_flags: u32) -> *mut Page {
    diagnostic(c"alloc_page_profile", -95);
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn put_page(_page: *mut Page) {
    require(false, c"put_page_profile");
}
#[unsafe(no_mangle)]
pub extern "C" fn page_address(_page: *const Page) -> *mut c_void {
    require(false, c"page_address_profile");
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn page_to_phys(_page: *const Page) -> usize {
    require(false, c"page_phys_profile");
    0
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_frag_page(fragment: *const SkbFrag) -> *mut Page {
    require(!fragment.is_null(), c"fragment_null");
    unsafe { (*fragment).page }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_frag_size(fragment: *const SkbFrag) -> u32 {
    require(!fragment.is_null(), c"fragment_null");
    unsafe { (*fragment).size }
}
#[unsafe(no_mangle)]
pub extern "C" fn skb_fill_page_desc(
    _skb: *mut SkBuff,
    _i: i32,
    _page: *mut Page,
    _off: i32,
    _size: i32,
) {
    require(false, c"fragment_packet_profile");
}
#[unsafe(no_mangle)]
pub extern "C" fn napi_get_frags(_napi: *mut Napi) -> *mut SkBuff {
    require(false, c"jumbo_napi_profile");
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn napi_gro_frags(_napi: *mut Napi) -> i32 {
    diagnostic(c"jumbo_ingress", -95)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pskb_trim(skb: *mut SkBuff, length: u32) -> i32 {
    require(!skb.is_null(), c"skb_metadata");
    unsafe {
        if (*skb).data_len != 0 {
            diagnostic(c"fragment_trim", -95)
        } else {
            packet::skb_trim(skb, length);
            0
        }
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn __pskb_pull_tail(_skb: *mut SkBuff, _len: i32) -> *mut c_void {
    require(false, c"fragment_pull_tail");
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn __vlan_hwaccel_put_tag(_skb: *mut SkBuff, _protocol: u16, _tag: u16) {
    require(false, c"vlan_profile");
}
#[unsafe(no_mangle)]
pub extern "C" fn csum_tcpudp_magic(
    _a: u32,
    _b: u32,
    _length: u32,
    _protocol: u16,
    _sum: u32,
) -> u16 {
    require(false, c"checksum_offload_profile");
    0
}
unsafe fn header(skb: *const SkBuff, offset: u32, size: usize) -> *mut c_void {
    unsafe {
        require(
            !skb.is_null() && (*skb).tail as usize >= (*skb).head as usize,
            c"skb_header_bounds",
        );
        let len = (*skb).tail as usize - (*skb).head as usize;
        require(
            offset as usize <= len && size <= len - offset as usize,
            c"skb_header_bounds",
        );
        let pointer = (*skb).head.add(offset as usize);
        checked(
            crate::driver_memory::range(pointer as usize, size, 0),
            c"skb_header_device_owned",
        );
        pointer.cast()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ip_hdr(skb: *const SkBuff) -> *mut c_void {
    unsafe { header(skb, (*skb).network_header, 20) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ipv6_hdr(skb: *const SkBuff) -> *mut c_void {
    unsafe { header(skb, (*skb).network_header, 40) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tcp_hdr(skb: *const SkBuff) -> *mut c_void {
    unsafe { header(skb, (*skb).transport_header, 20) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_network_offset(skb: *const SkBuff) -> u32 {
    unsafe {
        (*skb)
            .network_header
            .wrapping_sub(((*skb).data as usize).wrapping_sub((*skb).head as usize) as u32)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_transport_offset(skb: *const SkBuff) -> u32 {
    unsafe {
        (*skb)
            .transport_header
            .wrapping_sub(((*skb).data as usize).wrapping_sub((*skb).head as usize) as u32)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_tcp_all_headers(skb: *const SkBuff) -> u32 {
    unsafe { skb_transport_offset(skb).wrapping_add(kutox_tcp_header_bytes(tcp_hdr(skb))) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_checksum_start_offset(skb: *const SkBuff) -> u16 {
    unsafe { skb_transport_offset(skb) as u16 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_is_gso_v6(skb: *const SkBuff) -> bool {
    unsafe { packet::skb_is_gso(skb) && (*skb).protocol == 0x86ddu16.to_be() }
}
#[unsafe(no_mangle)]
pub extern "C" fn tcp_v6_gso_csum_prep(_skb: *mut SkBuff) {
    require(false, c"gso_profile");
}
#[unsafe(no_mangle)]
pub extern "C" fn skb_cow_head(_skb: *mut SkBuff, _headroom: u32) -> i32 {
    diagnostic(c"skb_cow_profile", -95)
}
#[unsafe(no_mangle)]
pub extern "C" fn ethtool_op_get_ts_info(_d: *mut NetDevice, _info: *mut c_void) -> i32 {
    diagnostic(c"ethtool_timestamp_profile", -95)
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ethtool_convert_legacy_u32_to_link_mode(bitmap: *mut usize, legacy: u32) {
    require(!bitmap.is_null(), c"ethtool_bitmap_null");
    unsafe {
        bitmap.write_bytes(0, 4);
        bitmap.write(legacy as usize);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ethtool_convert_link_mode_to_legacy_u32(
    legacy: *mut u32,
    bitmap: *const usize,
) -> bool {
    require(
        !legacy.is_null() && !bitmap.is_null(),
        c"ethtool_bitmap_null",
    );
    unsafe {
        if bitmap.read() > u32::MAX as usize
            || bitmap.add(1).read() != 0
            || bitmap.add(2).read() != 0
            || bitmap.add(3).read() != 0
        {
            return false;
        }
        legacy.write(bitmap.read() as u32);
        true
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn if_mii(_request: *mut c_void) -> *mut c_void {
    require(false, c"mii_ioctl_profile");
    ptr::null_mut()
}
