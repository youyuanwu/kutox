//! Kutox-owned C contracts shared by the Linux driver profiles.
use core::{
    ffi::{c_char, c_void},
    mem::{align_of, offset_of, size_of},
    ptr,
};

pub const GFP_KERNEL: u32 = 3264;
pub const GFP_ATOMIC: u32 = 2080;
pub const GFP_ZERO: u32 = 256;

#[repr(C)]
pub struct Device {
    pub parent: *mut Device,
    pub driver: *mut DeviceDriver,
    pub bus: *const Bus,
    pub groups: *const c_void,
    pub release: Option<unsafe extern "C" fn(*mut Device)>,
    pub driver_data: *mut c_void,
    pub of_node: *mut c_void,
}
impl Device {
    pub const EMPTY: Self = Self {
        parent: ptr::null_mut(),
        driver: ptr::null_mut(),
        bus: ptr::null(),
        groups: ptr::null(),
        release: None,
        driver_data: ptr::null_mut(),
        of_node: ptr::null_mut(),
    };
}
#[repr(C)]
pub struct DeviceDriver {
    pub name: *const c_char,
    pub bus: *const Bus,
    pub owner: *mut c_void,
}
#[repr(C)]
pub struct Bus {
    pub name: *const c_char,
    pub groups: *const c_void,
    pub matches: Option<unsafe extern "C" fn(*mut Device, *const DeviceDriver) -> i32>,
    pub uevent: Option<unsafe extern "C" fn(*const Device, *mut c_void) -> i32>,
    pub probe: Option<unsafe extern "C" fn(*mut Device) -> i32>,
    pub remove: Option<unsafe extern "C" fn(*mut Device)>,
    pub affinity: Option<unsafe extern "C" fn(*mut Device, u32) -> *const c_void>,
    pub shutdown: Option<unsafe extern "C" fn(*mut Device)>,
    pub num_vf: Option<unsafe extern "C" fn(*mut Device) -> i32>,
}
#[repr(C)]
pub struct PciDevice {
    pub dev: Device,
    pub vendor: u16,
    pub device: u16,
    pub subsystem_vendor: u16,
    pub subsystem_device: u16,
    pub revision: u8,
    pub irq: u32,
    pub is_virtfn: bool,
    pub physfn: *mut PciDevice,
}
impl PciDevice {
    pub const EMPTY: Self = Self {
        dev: Device::EMPTY,
        vendor: 0,
        device: 0,
        subsystem_vendor: 0,
        subsystem_device: 0,
        revision: 0,
        irq: 0,
        is_virtfn: false,
        physfn: ptr::null_mut(),
    };
}
#[repr(C)]
pub struct PciId {
    pub vendor: u32,
    pub device: u32,
    pub subvendor: u32,
    pub subdevice: u32,
    pub class: u32,
    pub class_mask: u32,
    pub driver_data: usize,
}
#[repr(C)]
pub struct PciDriver {
    pub name: *const c_char,
    pub ids: *const PciId,
    pub probe: Option<unsafe extern "C" fn(*mut PciDevice, *const PciId) -> i32>,
    pub remove: Option<unsafe extern "C" fn(*mut PciDevice)>,
    pub pm: *const c_void,
    pub shutdown: Option<unsafe extern "C" fn(*mut PciDevice)>,
    pub errors: *const c_void,
    pub sriov: Option<unsafe extern "C" fn(*mut PciDevice, i32) -> i32>,
}
#[repr(C)]
pub struct Mutex {
    pub boot_held: u8,
    pub generation: u64,
}
#[repr(C)]
pub struct Completion {
    pub reserved: u8,
    pub generation: u64,
}
#[repr(C)]
pub struct Spinlock {
    pub held: u8,
    pub owner: u64,
}
#[repr(C)]
pub struct Work {
    pub function: Option<unsafe extern "C" fn(*mut c_void)>,
    pub generation: u64,
}
#[repr(C)]
pub struct List {
    pub next: *mut List,
    pub prev: *mut List,
}
pub const LAYOUT: [usize; 37] = [
    size_of::<Device>(),
    align_of::<Device>(),
    offset_of!(Device, parent),
    offset_of!(Device, driver),
    offset_of!(Device, bus),
    offset_of!(Device, groups),
    offset_of!(Device, release),
    offset_of!(Device, driver_data),
    offset_of!(Device, of_node),
    size_of::<PciDevice>(),
    offset_of!(PciDevice, dev),
    offset_of!(PciDevice, vendor),
    offset_of!(PciDevice, device),
    offset_of!(PciDevice, subsystem_vendor),
    offset_of!(PciDevice, subsystem_device),
    offset_of!(PciDevice, revision),
    offset_of!(PciDevice, irq),
    offset_of!(PciDevice, is_virtfn),
    offset_of!(PciDevice, physfn),
    size_of::<PciDriver>(),
    offset_of!(PciDriver, probe),
    offset_of!(PciDriver, remove),
    offset_of!(PciDriver, pm),
    offset_of!(PciDriver, errors),
    offset_of!(PciDriver, sriov),
    size_of::<DeviceDriver>(),
    size_of::<Bus>(),
    offset_of!(Bus, probe),
    offset_of!(Bus, remove),
    size_of::<Mutex>(),
    size_of::<Completion>(),
    size_of::<Spinlock>(),
    size_of::<Work>(),
    size_of::<List>(),
    GFP_KERNEL as usize,
    GFP_ATOMIC as usize,
    GFP_ZERO as usize,
];
