use super::{Bsp, abi::*, execution::*, memory_ffi::*, mmio::*, napi::*, packet::*, pci::*};
use crate::{driver_memory as native, driver_platform, linux_deferred, linux_dma, linux_runtime};
use core::{
    cell::UnsafeCell,
    ffi::c_void,
    mem::size_of,
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

struct Endpoint {
    pci: *mut PciDevice,
    mmio: *mut c_void,
}
static ENDPOINT: Bsp<Endpoint> = Bsp(UnsafeCell::new(Endpoint {
    pci: ptr::null_mut(),
    mmio: ptr::null_mut(),
}));
fn pdev() -> *mut PciDevice {
    unsafe { (*ENDPOINT.0.get()).pci }
}
fn device() -> *mut Device {
    pdev().cast()
}
fn base() -> usize {
    unsafe { (*ENDPOINT.0.get()).mmio as usize }
}
fn fragment_size() -> u32 {
    (2114usize.next_multiple_of(64) + size_of::<SkbSharedInfo>().next_multiple_of(64)) as u32
}
unsafe extern "C" {
    fn kutox_c_packet_boundary(device: *mut NetDevice, skb: *mut SkBuff, consume: bool) -> bool;
}

pub(crate) fn begin(irq: u32) {
    unsafe {
        let p = driver_endpoint(irq);
        (*ENDPOINT.0.get()).pci = p;
        assert_eq!(pci_enable_device(p), 0);
        assert_eq!(pci_select_bars(p, 0x200), 1);
        assert_eq!(pci_request_selected_regions(p, 1, c"p3-test".as_ptr()), 0);
        pci_set_master(p);
        assert_eq!(pci_save_state(p), 0);
        let mmio = pci_ioremap_bar(p, 0);
        (*ENDPOINT.0.get()).mmio = mmio;
        assert!(!mmio.is_null());
        assert_eq!(dma_set_mask_and_coherent(device(), u32::MAX as u64), 0);
        let mut command = 0;
        assert_eq!(pci_read_config_word(p, 4, &mut command), 0);
        assert_eq!(command & 6, 6);
        assert_eq!(pci_resource_len(p, 0), 0x20000);
        assert_eq!(pci_resource_flags(p, 0), 0x200);
        assert_ne!(pci_resource_start(p, 0), 0);
        assert!(!pci_channel_offline(p));
        assert_eq!(pci_set_mwi(p), -5);
        pci_clear_mwi(p);
        device_set_wakeup_enable(device(), false);
        assert!(!device_can_wakeup(device()));
    }
}
pub(crate) fn end() {
    driver_platform::gate();
    unsafe {
        iounmap((*ENDPOINT.0.get()).mmio);
        (*ENDPOINT.0.get()).mmio = ptr::null_mut();
        pci_release_selected_regions(pdev(), 1);
        pci_disable_device(pdev());
        pci_set_drvdata(pdev(), ptr::null_mut());
        release_driver_endpoint();
        (*ENDPOINT.0.get()).pci = ptr::null_mut();
    }
}
pub(crate) fn allocations() {
    unsafe {
        assert!(kmalloc_array(usize::MAX, 2, GFP_KERNEL).is_null());
        native::fault(u32::MAX, 0);
        assert!(kzalloc(32, GFP_KERNEL).is_null());
        assert!(alloc_etherdev(32).is_null());
        native::fault(u32::MAX, usize::MAX);
        let object = kzalloc(33, GFP_KERNEL).cast::<u8>();
        assert!(!object.is_null());
        assert_eq!(object as usize & 63, 0);
        assert!(
            core::slice::from_raw_parts(object, 33)
                .iter()
                .all(|&b| b == 0)
        );
        object.add(32).write(0xa5);
        assert_eq!(native::size(object as usize), 33);
        assert_eq!(native::free(object as usize + 1), -16);
        kfree(object.cast());
        assert_eq!(native::free(object as usize), -22);
        let object = vmalloc(8193);
        assert!(!object.is_null());
        vfree(object);
        native::fault(u32::MAX, 1);
        assert!(alloc_etherdev(32).is_null());
        native::fault(u32::MAX, usize::MAX);
        let mut pressure = [ptr::null_mut(); 64];
        let mut count = 0;
        while count < pressure.len() {
            pressure[count] = kmalloc(4 * 1024 * 1024, GFP_KERNEL);
            if pressure[count].is_null() {
                break;
            }
            count += 1;
        }
        assert!(count > 0 && count < pressure.len());
        for &pointer in &pressure[..count] {
            kfree(pointer);
        }
        let object = kmalloc(32, GFP_KERNEL);
        assert!(!object.is_null());
        kfree(object);
    }
}
pub(crate) fn dma() {
    unsafe {
        let object = kmalloc(2048, GFP_KERNEL);
        assert!(!object.is_null());
        let invalid = dma_map_single(device(), object.byte_add(1), 2048, 2);
        assert!(dma_mapping_error(device(), invalid));
        let invalid = dma_map_single(device(), object, 2048, 0);
        assert!(dma_mapping_error(device(), invalid));
        let mapped = dma_map_single(device(), object, 2048, 2);
        assert!(!dma_mapping_error(device(), mapped));
        assert_eq!(dma_set_mask_and_coherent(device(), 0xffffff), -5);
        assert_eq!(dma_set_mask_and_coherent(device(), u64::MAX), -16);
        assert_eq!(native::free(object as usize), -16);
        assert_eq!(native::range(object as usize, 1, 0), -16);
        assert_eq!(
            native::dma_operation(device() as usize, mapped, 2047, 2, 0),
            -22
        );
        assert_eq!(
            native::dma_operation(device() as usize, mapped, 2048, 1, 0),
            -22
        );
        assert_eq!(native::dma_operation(1, mapped, 2048, 2, 0), -19);
        dma_sync_single_for_cpu(device(), mapped, 60, 2);
        assert_eq!(native::range(object as usize, 60, 0), 0);
        assert_eq!(native::range(object as usize + 60, 1, 0), -16);
        dma_sync_single_for_device(device(), mapped, 2048, 2);
        assert_eq!(native::range(object as usize, 1, 0), -16);
        dma_unmap_single(device(), mapped, 2048, 2);
        assert_eq!(
            native::dma_operation(device() as usize, mapped, 2048, 2, 0),
            -22
        );
        kfree(object);
        let mut address = 0xdead;
        native::fault(u32::MAX, 0);
        assert!(dma_alloc_coherent(device(), 4096, &mut address, GFP_KERNEL).is_null());
        assert_eq!(address, 0xdead);
        native::fault(u32::MAX, usize::MAX);
        let ring = dma_alloc_coherent(device(), 4096, &mut address, GFP_KERNEL);
        assert!(!ring.is_null());
        assert_eq!((address & 4095, ring as usize & 4095), (0, 0));
        ring.cast::<u32>().add(3).write_volatile(0x11223344);
        assert_eq!(ring.cast::<u32>().add(3).read_volatile(), 0x11223344);
        assert_eq!(
            linux_dma::free(device() as usize, 4095, ring as usize, address),
            -22
        );
        writel(2, (base() + 0x100) as *mut u32);
        assert_eq!(
            linux_dma::free(device() as usize, 4096, ring as usize, address),
            -16
        );
        assert_eq!(native::free(ring as usize), -16);
        driver_platform::gate();
        assert_eq!(readl((base() + 0x100) as *const u32) & 2, 0);
        dma_free_coherent(device(), 4096, ring, address);
    }
}
static ATOMIC_ALLOCATION: AtomicBool = AtomicBool::new(false);
unsafe extern "C" fn cancelled_work(_: *mut c_void) {
    panic!("embedded work not cancelled");
}
unsafe extern "C" fn unused_poll(napi: *mut Napi, _: i32) -> i32 {
    unsafe {
        assert_eq!(linux_runtime::context_code(), -58);
        assert!(kmalloc(32, GFP_KERNEL).is_null());
        let pointer = kmalloc(32, GFP_ATOMIC);
        assert!(!pointer.is_null());
        kfree(pointer);
        assert!(napi_complete_done(napi, 0));
        ATOMIC_ALLOCATION.store(true, Ordering::Release);
        0
    }
}
fn received(frame: &[u8]) {
    let mut output = [0; 1514];
    assert_eq!(native::receive(&mut output), frame.len());
    assert_eq!(&output[..frame.len()], frame);
}
pub(crate) fn packets() {
    unsafe {
        let net = alloc_etherdev(256);
        assert!(!net.is_null());
        let embedded = netdev_priv(net).cast::<Work>();
        assert_eq!(embedded as usize & 63, 0);
        assert!(kutox_c_packet_boundary(net, ptr::null_mut(), false));
        init_work(embedded, Some(cancelled_work));
        init_work(embedded.add(1), Some(cancelled_work));
        assert!(schedule_work(embedded.add(1)));
        assert_eq!(native::free(embedded as usize), -103);
        assert_eq!(
            linux_deferred::work_operation(embedded as usize, (*embedded).generation, 3, 0),
            0
        );
        assert!(cancel_work_sync(embedded.add(1)));
        mutex_init(embedded.byte_add(64).cast());
        set_netdev_dev(net, device());
        pci_set_drvdata(pdev(), net.cast());
        let mac = [0x52, 0x54, 0, 0x12, 0x34, 0x56];
        eth_hw_addr_set(net, mac.as_ptr());
        assert_eq!(eth_validate_addr(net), 0);
        assert_eq!(register_netdev(net), 0);
        assert!(!netif_running(net) && !netif_carrier_ok(net));
        assert_eq!(netdev_open(net), 0);
        assert!(netif_running(net) && !netif_queue_stopped(net));
        netif_carrier_on(net);
        netif_device_detach(net);
        assert!(netif_carrier_ok(net) && netif_xmit_stopped(netdev_get_tx_queue(net, 0)));
        netif_device_attach(net);
        netif_wake_queue(net);
        let skb = alloc_skb(10, GFP_ATOMIC);
        assert!(!skb.is_null());
        skb_put_data(skb, b"KUTOX".as_ptr().cast(), 5);
        assert_eq!(eth_skb_pad(skb), 0);
        assert_eq!(skb_headlen(skb), 60);
        assert!(
            core::slice::from_raw_parts((*skb).data.add(5), 55)
                .iter()
                .all(|&b| b == 0)
        );
        assert!(kutox_c_packet_boundary(net, skb, false));
        assert_eq!(netdev_transmit(net, skb), 16);
        assert_eq!((*skb).len, 60);
        (*skb_shinfo(skb)).gso_size = 64;
        assert_eq!(netdev_transmit(net, skb), -95);
        assert_eq!((*skb).len, 60);
        (*skb_shinfo(skb)).gso_size = 0;
        (*skb).no_fcs = 1;
        assert_eq!(netdev_transmit(net, skb), -95);
        (*skb).no_fcs = 0;
        assert_eq!(tx_begin(net, base(), (*skb).data.cast(), 60), 0);
        assert_eq!(tx_retry(net), 16);
        assert_eq!(tx_status(net), 0);
        tx_release(net);
        assert_eq!(tx_begin(net, base(), (*skb).data.cast(), 60), 0);
        assert!(kutox_c_packet_boundary(net, skb, true));
        assert_eq!(tx_retry(net), 0);
        assert_eq!(tx_status(net), -5);
        assert!(kutox_c_packet_boundary(net, skb, false));
        tx_release(net);
        netdev_sent_queue(net, 60);
        netdev_completed_queue(net, 1, 60);
        dev_kfree_skb_any(skb);
        let mut napi: Napi = core::mem::zeroed();
        netif_napi_add(net, &mut napi, Some(unused_poll));
        let mut ring_dma = 0;
        let ring = dma_alloc_coherent(device(), 4096, &mut ring_dma, GFP_KERNEL);
        assert!(!ring.is_null());
        napi_enable(&mut napi);
        assert_eq!(
            linux_dma::free(device() as usize, 4096, ring as usize, ring_dma),
            -16
        );
        assert!(napi_schedule_prep(&mut napi));
        __napi_schedule(&mut napi);
        for _ in 0..100 {
            if ATOMIC_ALLOCATION.load(Ordering::Acquire) {
                break;
            }
            msleep(1);
        }
        assert!(ATOMIC_ALLOCATION.load(Ordering::Acquire));
        napi_disable(&mut napi);
        ATOMIC_ALLOCATION.store(false, Ordering::Release);
        dma_free_coherent(device(), 4096, ring, ring_dma);
        let mut frame = [0; 60];
        frame[..6].fill(255);
        frame[6..12].copy_from_slice(&mac);
        frame[12..14].copy_from_slice(&[0x88, 0xb5]);
        frame[14..34].copy_from_slice(&b"KUTOX-SOURCE-SERVICES"[..20]);
        let skb = napi_alloc_skb(&mut napi, 60);
        assert!(!skb.is_null());
        skb_put_data(skb, frame.as_ptr().cast(), 60);
        assert_eq!(eth_type_trans(skb, net), 0x88b5u16.to_be());
        assert_eq!((*skb).len, 46);
        assert_eq!(napi_gro_receive(&mut napi, skb), 0);
        received(&frame);
        let fragment = netdev_alloc_frag(fragment_size());
        assert!(!fragment.is_null());
        ptr::copy_nonoverlapping(frame.as_ptr(), fragment.byte_add(66).cast(), frame.len());
        let dma = dma_map_single(device(), fragment.byte_add(66), 2048, 2);
        assert!(!dma_mapping_error(device(), dma));
        let skb = napi_build_skb(fragment, fragment_size());
        assert!(!skb.is_null());
        skb_reserve(skb, 66);
        dma_unmap_single(device(), dma, 2048, 2);
        skb_put(skb, 60);
        eth_type_trans(skb, net);
        napi_gro_receive(&mut napi, skb);
        received(&frame);
        for protocol in [[0, 1], [0x81, 0]] {
            frame[12..14].copy_from_slice(&protocol);
            let skb = napi_alloc_skb(&mut napi, 60);
            assert!(!skb.is_null());
            skb_put_data(skb, frame.as_ptr().cast(), 60);
            eth_type_trans(skb, net);
            assert_eq!(napi_gro_receive(&mut napi, skb), -95);
            let mut output = [0; 1514];
            assert_eq!(native::receive(&mut output), 0);
        }
        let skb = napi_alloc_skb(&mut napi, 1);
        assert!(!skb.is_null());
        assert_eq!(eth_type_trans(skb, net), 0);
        assert_eq!(napi_gro_receive(&mut napi, skb), -95);
        assert_eq!((*net).stats.rx_dropped, 3);
        let fragment = netdev_alloc_frag(fragment_size());
        assert!(!fragment.is_null());
        skb_free_frag(fragment.byte_add(66));
        napi_free_frags(&mut napi);
        netif_napi_del(&mut napi);
        unregister_netdev(net);
        free_netdev(net);
        linux_deferred::stop_workers();
    }
}
pub(crate) fn capacity() {
    unsafe {
        let mut rx = [ptr::null_mut(); 256];
        let mut tx = [ptr::null_mut(); 256];
        let mut mappings = [0; 256];
        let mut tx_mappings = [0; 256];
        let mut tx_dma = 0;
        let mut rx_dma = 0;
        let tx_ring = dma_alloc_coherent(device(), 4096, &mut tx_dma, GFP_KERNEL);
        let rx_ring = dma_alloc_coherent(device(), 4096, &mut rx_dma, GFP_KERNEL);
        assert!(!tx_ring.is_null() && !rx_ring.is_null());
        for i in 0..256 {
            rx[i] = netdev_alloc_frag(fragment_size());
            tx[i] = kmalloc(1514, GFP_ATOMIC);
            assert!(!rx[i].is_null() && !tx[i].is_null());
            mappings[i] = dma_map_single(device(), rx[i].byte_add(66), 2048, 2);
            tx_mappings[i] = dma_map_single(device(), tx[i], 1514, 1);
            assert!(!dma_mapping_error(device(), mappings[i]));
            assert!(!dma_mapping_error(device(), tx_mappings[i]));
        }
        writel(2, (base() + 0x100) as *mut u32);
        dma_unmap_single(device(), tx_mappings[0], 1514, 1);
        assert_ne!(readl((base() + 0x100) as *const u32) & 2, 0);
        assert_eq!(native::free(rx[0] as usize), -16);
        kfree(tx[0]);
        tx[0] = ptr::null_mut();
        driver_platform::gate();
        for i in 0..256 {
            dma_unmap_single(device(), mappings[i], 2048, 2);
            if !tx[i].is_null() {
                dma_unmap_single(device(), tx_mappings[i], 1514, 1);
            }
            skb_free_frag(rx[i].byte_add(66));
            kfree(tx[i]);
        }
        dma_free_coherent(device(), 4096, tx_ring, tx_dma);
        dma_free_coherent(device(), 4096, rx_ring, rx_dma);
    }
}
unsafe extern "C" fn unused_irq(_: i32, _: *mut c_void) -> i32 {
    0
}
pub(crate) fn failed_open() {
    unsafe {
        let rx = netdev_alloc_frag(2496);
        assert!(!rx.is_null());
        let dma = dma_map_single(device(), rx.byte_add(66), 2048, 2);
        assert!(!dma_mapping_error(device(), dma));
        writel(2, (base() + 0x100) as *mut u32);
        assert!(
            request_irq(
                100,
                Some(unused_irq),
                128,
                c"failed-open".as_ptr(),
                pdev().cast()
            ) < 0
        );
        assert_eq!(readl((base() + 0x100) as *const u32) & 2, 0);
        dma_unmap_single(device(), dma, 2048, 2);
        skb_free_frag(rx.byte_add(66));
    }
}
