//! Network-class services; descriptor programming and completion stay in Linux.
use super::memory_ffi as memory;
use super::*;
use core::{mem::size_of, ptr};

const ETH_HLEN: u32 = 14;
const TX_OK: i32 = 0;
const TX_BUSY: i32 = 0x10;

struct Tx {
    skb: *mut SkBuff,
    identity: usize,
    data: usize,
    mmio: usize,
    dma: u64,
    bytes: u32,
    completed_before: usize,
    completed_bytes_before: usize,
    stats_before: usize,
    stats_bytes_before: usize,
    live: bool,
    accepted: bool,
    mapped: bool,
    sent: bool,
    published: bool,
    freed: bool,
    completed: bool,
    aborting: bool,
    closing: bool,
}
impl Tx {
    const EMPTY: Self = Self {
        skb: ptr::null_mut(),
        identity: 0,
        data: 0,
        mmio: 0,
        dma: 0,
        bytes: 0,
        completed_before: 0,
        completed_bytes_before: 0,
        stats_before: 0,
        stats_bytes_before: 0,
        live: false,
        accepted: false,
        mapped: false,
        sent: false,
        published: false,
        freed: false,
        completed: false,
        aborting: false,
        closing: false,
    };
}
struct Many {
    live: bool,
    pending: *mut SkBuff,
}
impl Many {
    const EMPTY: Self = Self {
        live: false,
        pending: ptr::null_mut(),
    };
}
struct Rx {
    device: *mut NetDevice,
    mmio: usize,
    identity: usize,
    enabled: bool,
    active_skb: bool,
    copy: bool,
}
impl Rx {
    const EMPTY: Self = Self {
        device: ptr::null_mut(),
        mmio: 0,
        identity: 0,
        enabled: false,
        active_skb: false,
        copy: false,
    };
}
struct Packets {
    netdev: *mut NetDevice,
    queue: NetdevQueue,
    fragments: [*mut c_void; 512],
    tx: Tx,
    many: Many,
    rx: Rx,
}
static PACKETS: Bsp<Packets> = Bsp(UnsafeCell::new(Packets {
    netdev: ptr::null_mut(),
    queue: NetdevQueue {
        device: ptr::null_mut(),
    },
    fragments: [ptr::null_mut(); 512],
    tx: Tx::EMPTY,
    many: Many::EMPTY,
    rx: Rx::EMPTY,
}));

// Driver callbacks run on the BSP with IF clear and can reenter these services.
// Keep state access raw: no Rust borrow may span a driver or native callback.
fn state() -> *mut Packets {
    PACKETS.0.get()
}
fn tx_event(event: u32, result: i32) {
    unsafe { crate::linux_driver::trace(event, 0, (*state()).tx.identity, result) }
}
fn rx_event(event: u32, result: i32) {
    unsafe { crate::linux_driver::trace(event, 0, (*state()).rx.identity, result) }
}
fn netdev_check(device: *const NetDevice) {
    require(
        !device.is_null() && device == unsafe { (*state()).netdev },
        c"netdev_identity",
    );
}
fn data_align(size: usize) -> usize {
    (size + 63) & !63
}
fn skb_check(skb: *const SkBuff) {
    unsafe {
        require(
            !skb.is_null() && crate::driver_memory::size(skb as usize) == size_of::<SkBuff>(),
            c"skb_metadata",
        );
        require(
            (*skb).references != 0
                && (*skb).data as usize >= (*skb).head as usize
                && (*skb).tail as usize >= (*skb).data as usize
                && (*skb).end as usize >= (*skb).tail as usize
                && (*skb).data_len == 0
                && (*skb).len as usize == (*skb).tail as usize - (*skb).data as usize,
            c"skb_metadata",
        );
        let backing = crate::driver_memory::size((*skb).head as usize);
        require(
            backing >= size_of::<SkbSharedInfo>()
                && (*skb).end as usize - (*skb).head as usize
                    <= backing - size_of::<SkbSharedInfo>(),
            c"skb_backing_bounds",
        );
    }
}
pub(super) fn registered_netdev() -> *mut NetDevice {
    unsafe {
        let device = (*state()).netdev;
        if !device.is_null() && (*device).registered {
            device
        } else {
            ptr::null_mut()
        }
    }
}
pub(super) unsafe fn netdev_close(device: *mut NetDevice) -> i32 {
    unsafe {
        if device.is_null()
            || device != (*state()).netdev
            || !(*device).registered
            || !(*device).running
        {
            return -22;
        }
        (*device).running = false;
        callback((*(*device).netdev_ops).stop, c"netdev_stop_callback")(device)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn alloc_etherdev(private_size: usize) -> *mut NetDevice {
    unsafe {
        if !(*state()).netdev.is_null() || private_size == 0 || private_size > 65536 {
            diagnostic(c"netdev_allocation_profile", -22);
            return ptr::null_mut();
        }
        let device = memory::kzalloc(size_of::<NetDevice>(), GFP_KERNEL).cast::<NetDevice>();
        if device.is_null() {
            return device;
        }
        let private_data = memory::kzalloc(private_size, GFP_KERNEL);
        if private_data.is_null() {
            memory::kfree(device.cast());
            return ptr::null_mut();
        }
        (*device).private_data = private_data;
        (*device).private_size = private_size;
        (*device).mtu = 1500;
        (*device).addr_len = 6;
        (*device).stopped = true;
        ptr::copy_nonoverlapping(c"eth0".as_ptr().cast(), (*device).name.as_mut_ptr(), 5);
        (*state()).netdev = device;
        device
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free_netdev(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        require(
            !(*device).registered
                && !(*device).running
                && (*device).sent_packets == (*device).completed_packets
                && !(*state()).tx.live
                && !(*state()).rx.enabled
                && !(*state()).many.live,
            c"free_netdev_live",
        );
        napi::retire_device(device);
        pci::clear_data(device.cast());
        memory::kfree((*device).private_data);
        memory::kfree(device.cast());
        (*state()).netdev = ptr::null_mut();
        (*state()).queue.device = ptr::null_mut();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netdev_priv(device: *const NetDevice) -> *mut c_void {
    netdev_check(device);
    unsafe { (*device).private_data }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn set_netdev_dev(device: *mut NetDevice, parent: *mut Device) {
    netdev_check(device);
    require(
        parent as usize == { crate::driver_platform::device_key() },
        c"netdev_parent",
    );
    unsafe { (*device).parent = parent }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn register_netdev(device: *mut NetDevice) -> i32 {
    netdev_check(device);
    unsafe {
        let ops = (*device).netdev_ops;
        if (*device).registered
            || (*device).parent.is_null()
            || ops.is_null()
            || (*ops).open.is_none()
            || (*ops).stop.is_none()
            || (*ops).start_xmit.is_none()
            || (*device).mtu != 1500
            || eth_validate_addr(device) != 0
        {
            return -22;
        }
        let selected = match (*ops).fix_features {
            Some(fix) => fix(device, 0),
            None => 0,
        };
        if selected != 0 {
            return -95;
        }
        if let Some(set) = (*ops).set_features {
            let result = set(device, selected);
            if result < 0 {
                return result;
            }
        }
        (*device).features = selected;
        (*device).vlan_features = 0;
        (*device).registered = true;
        ptr::copy_nonoverlapping(c"eth0".as_ptr().cast(), (*device).name.as_mut_ptr(), 5);
        0
    }
}
pub(super) unsafe fn netdev_open(device: *mut NetDevice) -> i32 {
    netdev_check(device);
    unsafe {
        if !(*device).registered || (*device).running || (*device).detached {
            return -22;
        }
        (*device).running = true;
        let result = callback((*(*device).netdev_ops).open, c"netdev_open_callback")(device);
        if result < 0 {
            (*device).running = false;
            crate::driver_platform::gate();
        }
        result
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn unregister_netdev(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        require((*device).registered, c"netdev_unregister_unregistered");
        if (*device).running {
            (*device).running = false;
            checked(
                callback((*(*device).netdev_ops).stop, c"netdev_stop_callback")(device),
                c"netdev_stop",
            );
        }
        (*device).registered = false;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_carrier_off(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        (*device).carrier = false;
        crate::net_device::wake();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_carrier_on(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        (*device).carrier = true;
        crate::linux_driver::trace(14, 0, device as usize, 0);
        crate::net_device::wake();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_carrier_ok(device: *const NetDevice) -> bool {
    netdev_check(device);
    unsafe { (*device).carrier }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_running(device: *const NetDevice) -> bool {
    netdev_check(device);
    unsafe { (*device).running }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_device_detach(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        (*device).detached = true;
        (*device).stopped = true;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_device_attach(device: *mut NetDevice) {
    netdev_check(device);
    unsafe { (*device).detached = false }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_start_queue(device: *mut NetDevice) {
    netdev_check(device);
    unsafe { (*device).stopped = false }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_stop_queue(device: *mut NetDevice) {
    netdev_check(device);
    unsafe { (*device).stopped = true }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_wake_queue(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        if !(*device).detached && (*device).stopped {
            (*device).stopped = false;
            crate::net_device::wake();
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_tx_disable(device: *mut NetDevice) {
    unsafe { netif_stop_queue(device) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_queue_stopped(device: *const NetDevice) -> bool {
    netdev_check(device);
    unsafe { (*device).stopped || (*device).detached }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netdev_sent_queue(device: *mut NetDevice, bytes: u32) {
    netdev_check(device);
    unsafe {
        require(
            bytes != 0
                && (*device).sent_packets != usize::MAX
                && (*device).sent_bytes <= usize::MAX - bytes as usize,
            c"netdev_sent_overflow",
        );
        (*device).sent_packets += 1;
        (*device).sent_bytes += bytes as usize;
        if (*state()).many.live {
            crate::linux_driver::trace(48, 0, device as usize, bytes as i32);
        }
        if (*state()).tx.live && !(*state()).tx.aborting {
            require(
                (*state()).tx.mapped && !(*state()).tx.sent && bytes == (*state()).tx.bytes,
                c"tx_sent_order",
            );
            (*state()).tx.sent = true;
            tx_event(20, bytes as i32);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netdev_completed_queue(device: *mut NetDevice, packets: u32, bytes: u32) {
    netdev_check(device);
    unsafe {
        require(
            packets as usize
                <= (*device)
                    .sent_packets
                    .wrapping_sub((*device).completed_packets)
                && bytes as usize <= (*device).sent_bytes.wrapping_sub((*device).completed_bytes),
            c"netdev_completion_accounting",
        );
        (*device).completed_packets = (*device).completed_packets.wrapping_add(packets as usize);
        (*device).completed_bytes = (*device).completed_bytes.wrapping_add(bytes as usize);
        if (*state()).many.live && packets != 0 {
            crate::linux_driver::trace(49, 0, device as usize, packets as i32);
        }
        if (*state()).tx.live && packets != 0 && !(*state()).tx.aborting {
            require(
                (*state()).tx.accepted
                    && (*state()).tx.published
                    && !(*state()).tx.mapped
                    && (*state()).tx.freed
                    && !(*state()).tx.completed
                    && packets == 1
                    && bytes == (*state()).tx.bytes,
                c"tx_completed_order",
            );
            (*state()).tx.completed = true;
            tx_event(24, bytes as i32);
            crate::net_device::wake();
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netdev_reset_queue(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        require((*device).stopped, c"reset_running_queue");
        (*device).sent_packets = 0;
        (*device).completed_packets = 0;
        (*device).sent_bytes = 0;
        (*device).completed_bytes = 0;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netdev_get_tx_queue(
    device: *const NetDevice,
    index: u32,
) -> *mut NetdevQueue {
    netdev_check(device);
    require(index == 0, c"netdev_queue_index");
    unsafe {
        (*state()).queue.device = (*state()).netdev;
        ptr::addr_of_mut!((*state()).queue)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_xmit_stopped(queue: *const NetdevQueue) -> bool {
    unsafe {
        require(
            queue == ptr::addr_of!((*state()).queue),
            c"netdev_queue_identity",
        );
        netif_queue_stopped((*queue).device)
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn netdev_xmit_more() -> bool {
    false
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netif_trans_update(device: *mut NetDevice) {
    netdev_check(device);
    unsafe { (*device).lifecycle = (*device).lifecycle.wrapping_add(1) }
}
#[unsafe(no_mangle)]
pub extern "C" fn netdev_uc_count(device: *const NetDevice) -> u32 {
    netdev_check(device);
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn netdev_mc_count(device: *const NetDevice) -> u32 {
    netdev_check(device);
    0
}
#[unsafe(no_mangle)]
pub extern "C" fn netdev_uc_first(device: *const NetDevice) -> *mut NetdevHwAddr {
    netdev_check(device);
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub extern "C" fn netdev_mc_first(device: *const NetDevice) -> *mut NetdevHwAddr {
    netdev_check(device);
    ptr::null_mut()
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn netdev_addr_next(address: *mut NetdevHwAddr) -> *mut NetdevHwAddr {
    require(!address.is_null(), c"address_iterator_null");
    unsafe { (*address).next }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn is_multicast_ether_addr(address: *const u8) -> bool {
    require(!address.is_null(), c"ethernet_null");
    unsafe { address.read() & 1 != 0 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn is_broadcast_ether_addr(address: *const u8) -> bool {
    require(!address.is_null(), c"ethernet_null");
    (0..6).all(|i| unsafe { address.add(i).read() == 255 })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn is_valid_ether_addr(address: *const u8) -> bool {
    unsafe { !is_multicast_ether_addr(address) && (0..6).any(|i| address.add(i).read() != 0) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eth_hw_addr_set(device: *mut NetDevice, address: *const u8) {
    netdev_check(device);
    require(!address.is_null(), c"ethernet_null");
    unsafe { ptr::copy_nonoverlapping(address, (*device).dev_addr.as_mut_ptr(), 6) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eth_validate_addr(device: *mut NetDevice) -> i32 {
    netdev_check(device);
    if unsafe { is_valid_ether_addr((*device).dev_addr.as_ptr()) } {
        0
    } else {
        -99
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn alloc_skb(size: u32, flags: u32) -> *mut SkBuff {
    if size == 0 || size > 4096 {
        diagnostic(c"skb_profile", -22);
        return ptr::null_mut();
    }
    let skb = memory::kzalloc(size_of::<SkBuff>(), flags).cast::<SkBuff>();
    if skb.is_null() {
        return skb;
    }
    let capacity = data_align(size as usize) + data_align(size_of::<SkbSharedInfo>());
    let head = memory::kzalloc(capacity, flags).cast::<u8>();
    if head.is_null() {
        unsafe { memory::kfree(skb.cast()) };
        return ptr::null_mut();
    }
    unsafe {
        (*skb).head = head;
        (*skb).data = head;
        (*skb).tail = head;
        (*skb).end = head.add(data_align(size as usize));
        (*skb).truesize = (capacity + size_of::<SkBuff>()) as u32;
        (*skb).references = 1;
    }
    skb
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_shinfo(skb: *mut SkBuff) -> *mut SkbSharedInfo {
    skb_check(skb);
    unsafe { (*skb).end.cast() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_is_gso(skb: *const SkBuff) -> bool {
    unsafe { (*skb_shinfo(skb.cast_mut())).gso_size != 0 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_headlen(skb: *const SkBuff) -> u32 {
    skb_check(skb);
    unsafe { (*skb).len }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_reserve(skb: *mut SkBuff, size: u32) {
    skb_check(skb);
    unsafe {
        require(
            (*skb).len == 0 && size as usize <= (*skb).end as usize - (*skb).tail as usize,
            c"skb_reserve",
        );
        (*skb).data = (*skb).data.add(size as usize);
        (*skb).tail = (*skb).tail.add(size as usize);
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_put(skb: *mut SkBuff, size: u32) -> *mut c_void {
    skb_check(skb);
    unsafe {
        require(
            size as usize <= (*skb).end as usize - (*skb).tail as usize,
            c"skb_put",
        );
        let tail = (*skb).tail;
        if size != 0 {
            checked(
                crate::driver_memory::range(tail as usize, size as usize, 0),
                c"skb_put_device_owned",
            );
        }
        (*skb).tail = tail.add(size as usize);
        (*skb).len += size;
        tail.cast()
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_put_data(skb: *mut SkBuff, data: *const c_void, size: u32) {
    require(!data.is_null() || size == 0, c"skb_copy_null");
    unsafe {
        let tail = skb_put(skb, size);
        if size != 0 {
            ptr::copy_nonoverlapping(data.cast::<u8>(), tail.cast(), size as usize);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_tail_pointer(skb: *const SkBuff) -> *mut c_void {
    skb_check(skb);
    unsafe { (*skb).tail.cast() }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_tailroom(skb: *const SkBuff) -> u32 {
    skb_check(skb);
    unsafe { ((*skb).end as usize - (*skb).tail as usize) as u32 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_trim(skb: *mut SkBuff, size: u32) {
    skb_check(skb);
    unsafe {
        if size > (*skb).len {
            return;
        }
        (*skb).tail = (*skb).data.add(size as usize);
        (*skb).len = size;
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_pull(skb: *mut SkBuff, size: u32) -> *mut c_void {
    skb_check(skb);
    unsafe {
        if size > (*skb).len {
            return ptr::null_mut();
        }
        (*skb).data = (*skb).data.add(size as usize);
        (*skb).len -= size;
        (*skb).data.cast()
    }
}
unsafe fn free_skb(skb: *mut SkBuff) {
    skb_check(skb);
    unsafe {
        let tracked =
            (*state()).tx.live && !(*state()).tx.freed && skb as usize == (*state()).tx.identity;
        let received = (*state()).rx.enabled
            && (*state()).rx.active_skb
            && skb as usize == (*state()).rx.identity;
        let backing = (*skb).backing_kind;
        require((*skb).references == 1, c"skb_reference_profile");
        if backing != 0 {
            skb_free_frag((*skb).head.cast());
        } else {
            memory::kfree((*skb).head.cast());
        }
        (*skb).references = 0;
        memory::kfree(skb.cast());
        if tracked {
            require(
                !(*state()).tx.mapped && !(*state()).tx.freed,
                c"tx_free_order",
            );
            (*state()).tx.freed = true;
            (*state()).tx.skb = ptr::null_mut();
            crate::net_device::wake();
            if !(*state()).tx.sent || (*state()).tx.aborting {
                tx_event(28, 0);
            }
        }
        if received {
            (*state()).rx.active_skb = false;
            rx_event(42, backing as i32);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dev_kfree_skb(skb: *mut SkBuff) {
    if !skb.is_null() {
        unsafe { free_skb(skb) }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dev_kfree_skb_any(skb: *mut SkBuff) {
    unsafe { dev_kfree_skb(skb) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_consume_skb(skb: *mut SkBuff, budget: i32) {
    require((0..=64).contains(&budget), c"skb_consume_budget");
    unsafe {
        let tracked =
            (*state()).tx.live && !(*state()).tx.freed && skb as usize == (*state()).tx.identity;
        dev_kfree_skb(skb);
        if tracked && !(*state()).tx.aborting {
            require(
                (*state()).tx.accepted
                    && (*state()).tx.published
                    && (*state()).tx.freed
                    && budget == 64,
                c"tx_consume_order",
            );
            tx_event(23, budget);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eth_skb_pad(skb: *mut SkBuff) -> i32 {
    skb_check(skb);
    unsafe {
        if (*skb).len >= 60 {
            return 0;
        }
        let padding = 60 - (*skb).len;
        if skb_tailroom(skb) >= padding {
            skb_put(skb, padding).write_bytes(0, padding as usize);
            return 0;
        }
        let headroom = (*skb).data as usize - (*skb).head as usize;
        let capacity = data_align(headroom + 60) + data_align(size_of::<SkbSharedInfo>());
        let head = memory::kzalloc(capacity, GFP_ATOMIC).cast::<u8>();
        if head.is_null() {
            free_skb(skb);
            return -12;
        }
        checked(
            crate::driver_memory::range((*skb).data as usize, (*skb).len as usize, 0),
            c"pad_device_owned",
        );
        ptr::copy_nonoverlapping((*skb).data, head.add(headroom), (*skb).len as usize);
        if (*skb).backing_kind != 0 {
            skb_free_frag((*skb).head.cast());
        } else {
            memory::kfree((*skb).head.cast());
        }
        (*skb).head = head;
        (*skb).data = head.add(headroom);
        (*skb).tail = (*skb).data.add((*skb).len as usize);
        if (*state()).tx.live && skb as usize == (*state()).tx.identity {
            (*state()).tx.data = (*skb).data as usize;
        }
        (*skb).end = head.add(data_align(headroom + 60));
        (*skb).backing_kind = 0;
        (*skb).truesize = (capacity + size_of::<SkBuff>()) as u32;
        skb_put(skb, padding).write_bytes(0, padding as usize);
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn skb_tx_timestamp(skb: *mut SkBuff) {
    skb_check(skb);
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_checksum_none_assert(skb: *const SkBuff) {
    skb_check(skb);
    require(unsafe { (*skb).ip_summed == 0 }, c"skb_checksum_profile");
}
pub(super) unsafe fn netdev_transmit(device: *mut NetDevice, skb: *mut SkBuff) -> i32 {
    netdev_check(device);
    unsafe {
        if !(*device).registered
            || !(*device).running
            || !(*device).carrier
            || (*device).detached
            || (*device).stopped
        {
            return TX_BUSY;
        }
        skb_check(skb);
        let info = skb_shinfo(skb);
        checked(
            crate::driver_memory::range((*skb).data as usize, (*skb).len as usize, 0),
            c"transmit_device_owned",
        );
        if (*skb).len < ETH_HLEN
            || (*skb).len > 1514
            || (*skb).no_fcs != 0
            || (*info).gso_size != 0
            || (*info).nr_frags != 0
            || (*skb).ip_summed != 0
            || (*skb).vlan_present != 0
            || ((*skb).data.add(12).read() == 0x81 && (*skb).data.add(13).read() == 0)
            || ((*skb).data.add(12).read() == 0x88 && (*skb).data.add(13).read() == 0xa8)
        {
            return diagnostic(c"transmit_profile", -95);
        }
        let tracked =
            ((*state()).tx.live && skb as usize == (*state()).tx.identity) || (*state()).many.live;
        let callback = callback((*(*device).netdev_ops).start_xmit, c"netdev_xmit_callback");
        let identity = skb as usize;
        if tracked {
            crate::linux_driver::trace(17, callback as usize, identity, (*skb).len as i32);
        }
        let result = callback(skb, device);
        require(result == TX_OK || result == TX_BUSY, c"transmit_return");
        if tracked {
            crate::linux_driver::trace(18, callback as usize, identity, result);
        }
        result
    }
}
unsafe fn copy_tx_skb(device: *mut NetDevice, bytes: *const u8, size: u32) -> *mut SkBuff {
    let skb = alloc_skb(size, GFP_KERNEL);
    if skb.is_null() {
        return skb;
    }
    unsafe {
        skb_put_data(skb, bytes.cast(), size);
        (*skb).device = device;
        (*skb).protocol = u16::from_be_bytes([bytes.add(12).read(), bytes.add(13).read()]).to_be();
    }
    skb
}
pub(super) unsafe fn tx_begin(
    device: *mut NetDevice,
    mmio: usize,
    bytes: *const c_void,
    size: u32,
) -> i32 {
    netdev_check(device);
    unsafe {
        if (*state()).tx.live || (*state()).many.live {
            return -16;
        }
        if bytes.is_null() || mmio == 0 || !(ETH_HLEN..=1514).contains(&size) {
            return diagnostic(c"tx_packet_profile", -22);
        }
        let skb = copy_tx_skb(device, bytes.cast(), size);
        if skb.is_null() {
            return -12;
        }
        (*state()).tx = Tx {
            skb,
            identity: skb as usize,
            data: (*skb).data as usize,
            mmio,
            bytes: size.max(60),
            completed_before: (*device).completed_packets,
            completed_bytes_before: (*device).completed_bytes,
            stats_before: (*device).stats.tx_packets,
            stats_bytes_before: (*device).stats.tx_bytes,
            live: true,
            ..Tx::EMPTY
        };
        tx_event(31, size as i32);
        0
    }
}
pub(super) unsafe fn tx_retry(device: *mut NetDevice) -> i32 {
    netdev_check(device);
    unsafe {
        if !(*state()).tx.live
            || (*state()).tx.accepted
            || (*state()).tx.freed
            || (*state()).tx.aborting
        {
            return -22;
        }
        let result = netdev_transmit(device, (*state()).tx.skb);
        if result == TX_OK {
            (*state()).tx.accepted = true;
            (*state()).tx.skb = ptr::null_mut();
            require(
                (*state()).tx.freed
                    || ((*state()).tx.mapped && (*state()).tx.sent && (*state()).tx.published),
                c"tx_acceptance_order",
            );
        } else {
            require(
                !(*state()).tx.mapped && !(*state()).tx.sent && !(*state()).tx.freed,
                c"tx_busy_ownership",
            );
        }
        if result == TX_BUSY {
            tx_event(32, result);
        }
        result
    }
}
pub(super) unsafe fn tx_status(device: *mut NetDevice) -> i32 {
    netdev_check(device);
    unsafe {
        if !(*state()).tx.live || (*state()).tx.aborting {
            return -22;
        }
        if !(*state()).tx.accepted {
            return 0;
        }
        if (*state()).tx.completed {
            require(
                (*device).completed_packets == (*state()).tx.completed_before.wrapping_add(1)
                    && (*device).completed_bytes
                        == (*state())
                            .tx
                            .completed_bytes_before
                            .wrapping_add((*state()).tx.bytes as usize)
                    && (*device).stats.tx_packets == (*state()).tx.stats_before.wrapping_add(1)
                    && (*device).stats.tx_bytes
                        == (*state())
                            .tx
                            .stats_bytes_before
                            .wrapping_add((*state()).tx.bytes as usize),
                c"tx_source_stats",
            );
            return 2;
        }
        if (*state()).tx.freed { -5 } else { 1 }
    }
}
pub(super) unsafe fn tx_release(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        require(
            (*state()).tx.live && !(*state()).tx.mapped,
            c"tx_release_live_dma",
        );
        if !(*state()).tx.accepted {
            tx_event(29, 0);
            dev_kfree_skb((*state()).tx.skb);
        } else {
            require(
                (*state()).tx.freed
                    && ((*state()).tx.completed || (*state()).tx.aborting || !(*state()).tx.sent),
                c"tx_release_inflight",
            );
            if (*state()).tx.completed && !(*state()).tx.aborting {
                tx_event(25, (*state()).tx.bytes as i32);
            }
        }
        (*state()).tx = Tx::EMPTY;
    }
}
pub(super) fn tx_abort(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        if (*state()).tx.live {
            (*state()).tx.closing = true;
        }
        if (*state()).tx.live && (*state()).tx.accepted && !(*state()).tx.completed {
            (*state()).tx.aborting = true;
            tx_event(30, 0);
        }
    }
}
pub(super) fn tx_mapped(data: usize, dma: u64, size: usize, direction: u32) {
    unsafe {
        if !(*state()).tx.live || data != (*state()).tx.data || direction != 1 || dma == u64::MAX {
            return;
        }
        require(
            !(*state()).tx.mapped && !(*state()).tx.freed && size == (*state()).tx.bytes as usize,
            c"tx_map_order",
        );
        (*state()).tx.dma = dma;
        (*state()).tx.mapped = true;
        tx_event(19, size as i32);
    }
}
pub(super) fn tx_unmapped(dma: u64, size: usize, direction: u32) {
    unsafe {
        if !(*state()).tx.live || dma != (*state()).tx.dma || direction != 1 {
            return;
        }
        require(
            (*state()).tx.mapped
                && size == (*state()).tx.bytes as usize
                && ((*state()).tx.published || !(*state()).tx.sent),
            c"tx_unmap_order",
        );
        (*state()).tx.mapped = false;
        tx_event(22, size as i32);
    }
}
pub(super) fn tx_mmio(address: usize, value: u32, writing: bool) {
    unsafe {
        if !(*state()).tx.live || (*state()).tx.closing {
            return;
        }
        if writing
            && address == (*state()).tx.mmio + 0x3818
            && (*state()).tx.sent
            && !(*state()).tx.aborting
        {
            require(
                (*state()).tx.mapped && !(*state()).tx.published && value < 256,
                c"tx_doorbell_order",
            );
            (*state()).tx.published = true;
            tx_event(21, value as i32);
        } else if !writing && address == (*state()).tx.mmio + 0xc0 {
            tx_event(27, value as i32);
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_vlan_tag_present(skb: *const SkBuff) -> bool {
    skb_check(skb);
    unsafe { (*skb).vlan_present != 0 }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn skb_vlan_tag_get(skb: *const SkBuff) -> u16 {
    skb_check(skb);
    unsafe {
        require((*skb).vlan_present != 0, c"skb_vlan_absent");
        (*skb).vlan_tci
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vlan_get_protocol(skb: *const SkBuff) -> u16 {
    skb_check(skb);
    unsafe { (*skb).protocol }
}
#[unsafe(no_mangle)]
pub extern "C" fn netdev_alloc_frag(size: u32) -> *mut c_void {
    if size == 0 || size > 4096 {
        diagnostic(c"fragment_profile", -22);
        return ptr::null_mut();
    }
    unsafe {
        let slot = (0..512).find(|&slot| (*state()).fragments[slot].is_null());
        let Some(slot) = slot else {
            diagnostic(c"fragment_capacity", -12);
            return ptr::null_mut();
        };
        let pointer = crate::driver_memory::allocate(size as usize, GFP_ATOMIC, 1) as *mut c_void;
        if !pointer.is_null() {
            (*state()).fragments[slot] = pointer;
        } else if (*state()).rx.enabled && (*state()).rx.identity != 0 {
            rx_event(46, -12);
        }
        pointer
    }
}
fn fragment_slot(pointer: *const c_void) -> usize {
    unsafe {
        for slot in 0..512 {
            let base = (*state()).fragments[slot] as usize;
            if base != 0
                && pointer as usize >= base
                && (pointer as usize) - base < crate::driver_memory::size(base)
            {
                return slot;
            }
        }
        crate::platform_fail(c"fragment_provenance")
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn skb_free_frag(pointer: *mut c_void) {
    let slot = fragment_slot(pointer);
    unsafe {
        checked(
            crate::driver_memory::free((*state()).fragments[slot] as usize),
            c"fragment_free_pinned",
        );
        (*state()).fragments[slot] = ptr::null_mut();
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_build_skb(pointer: *mut c_void, size: u32) -> *mut SkBuff {
    let slot = fragment_slot(pointer);
    unsafe {
        require(
            (*state()).fragments[slot] == pointer
                && crate::driver_memory::size(pointer as usize) == size as usize
                && size as usize >= data_align(size_of::<SkbSharedInfo>()),
            c"build_skb_provenance",
        );
        let skb = memory::kzalloc(size_of::<SkBuff>(), GFP_ATOMIC).cast::<SkBuff>();
        if skb.is_null() {
            return skb;
        }
        checked(
            crate::driver_memory::range(pointer as usize, size as usize, 1),
            c"build_skb_claim",
        );
        (*skb).head = pointer.cast();
        (*skb).data = pointer.cast();
        (*skb).tail = pointer.cast();
        (*skb).end = pointer
            .cast::<u8>()
            .add(size as usize - data_align(size_of::<SkbSharedInfo>()));
        checked(
            crate::driver_memory::range((*skb).end as usize, size_of::<SkbSharedInfo>(), 0),
            c"build_skb_sharedinfo_owned",
        );
        (*skb).end.write_bytes(0, size_of::<SkbSharedInfo>());
        (*skb).references = 1;
        (*skb).backing_kind = 1;
        (*skb).truesize = size.wrapping_add(size_of::<SkBuff>() as u32);
        if (*state()).rx.enabled {
            require(!(*state()).rx.active_skb, c"rx_overlapping_skb");
            (*state()).rx.identity = skb as usize;
            (*state()).rx.active_skb = true;
            (*state()).rx.copy = false;
            rx_event(37, size as i32);
        }
        skb
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_alloc_skb(napi: *mut Napi, size: u32) -> *mut SkBuff {
    unsafe {
        require(
            !napi.is_null() && (*napi).device == (*state()).netdev,
            c"napi_skb_identity",
        );
        let skb = alloc_skb(size.wrapping_add(66), GFP_ATOMIC);
        if !skb.is_null() {
            skb_reserve(skb, 66);
        }
        if !skb.is_null() && (*state()).rx.enabled {
            require(!(*state()).rx.active_skb, c"rx_overlapping_skb");
            (*state()).rx.identity = skb as usize;
            (*state()).rx.active_skb = true;
            (*state()).rx.copy = true;
            rx_event(36, size as i32);
        }
        skb
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_free_frags(napi: *mut Napi) {
    require(
        unsafe { !napi.is_null() && (*napi).device == (*state()).netdev },
        c"napi_frags_identity",
    );
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn eth_type_trans(skb: *mut SkBuff, device: *mut NetDevice) -> u16 {
    netdev_check(device);
    skb_check(skb);
    unsafe {
        (*skb).mac_header = ((*skb).data as usize - (*skb).head as usize) as u32;
        if (*skb).len < ETH_HLEN {
            diagnostic(c"ethernet_short", -22);
            (*skb).device = device;
            (*skb).protocol = 0;
            return 0;
        }
        checked(
            crate::driver_memory::range((*skb).data as usize, (*skb).len as usize, 0),
            c"ethernet_device_owned",
        );
        (*skb).device = device;
        let protocol = u16::from_be_bytes([(*skb).data.add(12).read(), (*skb).data.add(13).read()]);
        skb_pull(skb, ETH_HLEN);
        if (*state()).rx.enabled && skb as usize == (*state()).rx.identity {
            rx_event(40, protocol as i32);
        }
        (*skb).protocol = protocol.to_be();
        (*skb).protocol
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn napi_gro_receive(napi: *mut Napi, skb: *mut SkBuff) -> i32 {
    unsafe {
        require(
            !napi.is_null()
                && (*napi).device == (*state()).netdev
                && !skb.is_null()
                && (*skb).device == (*state()).netdev,
            c"ingress_identity",
        );
        skb_check(skb);
        let frame = (*skb).head.add((*skb).mac_header as usize);
        let size = ((*skb).tail as usize).wrapping_sub(frame as usize);
        let protocol = u16::from_be((*skb).protocol);
        let unsupported = !(14..=1514).contains(&size)
            || (*skb).vlan_present != 0
            || (*skb).ip_summed != 0
            || protocol < 1536
            || protocol == 0x8100
            || protocol == 0x88a8;
        if size != 0 {
            checked(
                crate::driver_memory::range(frame as usize, size, 0),
                c"ingress_device_owned",
            );
        }
        let result = if unsupported {
            diagnostic(c"ingress_profile", -95)
        } else {
            crate::driver_memory::ingress(frame as usize, size)
        };
        if result < 0 {
            (*(*state()).netdev).stats.rx_dropped =
                (*(*state()).netdev).stats.rx_dropped.wrapping_add(1);
        }
        if (*state()).rx.enabled && skb as usize == (*state()).rx.identity {
            rx_event(
                if result < 0 { 44 } else { 41 },
                if result < 0 { result } else { size as i32 },
            );
        }
        free_skb(skb);
        result
    }
}
pub(crate) fn quiescent() -> bool {
    unsafe {
        (0..512).all(|slot| (*state()).fragments[slot].is_null())
            && (*state()).netdev.is_null()
            && (*state()).queue.device.is_null()
            && !(*state()).tx.live
            && !(*state()).rx.enabled
            && !(*state()).many.live
    }
}
pub(super) unsafe fn tx_many_retry(device: *mut NetDevice) -> i32 {
    netdev_check(device);
    unsafe {
        if !(*state()).many.live || (*state()).many.pending.is_null() {
            return -22;
        }
        let before = (*device).sent_packets;
        let result = netdev_transmit(device, (*state()).many.pending);
        if result == TX_OK {
            (*state()).many.pending = ptr::null_mut();
            if (*device).sent_packets != before.wrapping_add(1) {
                return diagnostic(c"tx_many_consumed_drop", -5);
            }
        } else if result < 0 {
            dev_kfree_skb((*state()).many.pending);
            (*state()).many.pending = ptr::null_mut();
        }
        result
    }
}
pub(super) unsafe fn tx_many(
    device: *mut NetDevice,
    bytes: *const c_void,
    count: u32,
    size: u32,
) -> i32 {
    netdev_check(device);
    unsafe {
        if (*state()).tx.live || (*state()).many.live || (*state()).rx.enabled {
            return -16;
        }
        if bytes.is_null() || count == 0 || count > 256 || size != 60 {
            return diagnostic(c"tx_many_profile", -22);
        }
        (*state()).many.live = true;
        for i in 0..count {
            (*state()).many.pending =
                copy_tx_skb(device, bytes.cast::<u8>().add((i * size) as usize), size);
            if (*state()).many.pending.is_null() {
                return -12;
            }
            let result = tx_many_retry(device);
            if result == TX_BUSY {
                return i as i32;
            }
            if result < 0 {
                return result;
            }
        }
        count as i32
    }
}
pub(super) unsafe fn tx_many_finish(device: *mut NetDevice) {
    netdev_check(device);
    unsafe {
        require(
            (*state()).many.live && (*device).sent_packets == (*device).completed_packets,
            c"tx_many_finish_live",
        );
        if !(*state()).many.pending.is_null() {
            dev_kfree_skb((*state()).many.pending);
        }
        (*state()).many = Many::EMPTY;
    }
}
pub(super) fn rx_start(device: *mut NetDevice, mmio: usize) {
    netdev_check(device);
    unsafe {
        require(
            !(*state()).rx.enabled && !(*state()).tx.live && mmio != 0,
            c"rx_start_state",
        );
        (*state()).rx = Rx {
            device,
            mmio,
            enabled: true,
            ..Rx::EMPTY
        };
    }
}
pub(super) fn rx_stop() {
    unsafe {
        require(
            (*state()).rx.enabled && !(*state()).rx.active_skb,
            c"rx_stop_state",
        );
        (*state()).rx = Rx::EMPTY;
    }
}
pub(super) fn rx_path() -> i32 {
    unsafe {
        if !(*state()).rx.enabled || (*state()).rx.active_skb || (*state()).rx.identity == 0 {
            return -22;
        }
        if (*state()).rx.copy { 1 } else { 2 }
    }
}
pub(super) fn rx_dma(dma: u64, size: usize, event: u32) {
    unsafe {
        if !(*state()).rx.enabled || (*state()).rx.identity == 0 {
            return;
        }
        require(
            dma != 0 && (event == 43 || (*state()).rx.active_skb),
            c"rx_dma_order",
        );
        rx_event(event, size as i32);
    }
}
pub(super) fn rx_mmio(address: usize, value: u32, writing: bool) {
    unsafe {
        if !(*state()).rx.enabled {
            return;
        }
        if !writing && address == (*state()).rx.mmio + 0xc0 {
            crate::linux_driver::trace(47, 0, (*state()).rx.device as usize, value as i32);
        } else if writing && address == (*state()).rx.mmio + 0x2818 && (*state()).rx.identity != 0 {
            rx_event(45, value as i32);
        }
    }
}

#[cfg(feature = "guest-tests")]
mod guest_ffi {
    use super::*;

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_c_netdev_open(device: *mut NetDevice) -> i32 {
        unsafe { netdev_open(device) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_c_netdev_transmit(device: *mut NetDevice, skb: *mut SkBuff) -> i32 {
        unsafe { netdev_transmit(device, skb) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_c_tx_begin(
        device: *mut NetDevice,
        mmio: usize,
        bytes: *const c_void,
        size: u32,
    ) -> i32 {
        unsafe { tx_begin(device, mmio, bytes, size) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_c_tx_retry(device: *mut NetDevice) -> i32 {
        unsafe { tx_retry(device) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_c_tx_status(device: *mut NetDevice) -> i32 {
        unsafe { tx_status(device) }
    }

    #[unsafe(no_mangle)]
    unsafe extern "C" fn kutox_c_tx_release(device: *mut NetDevice) {
        unsafe { tx_release(device) }
    }
}
