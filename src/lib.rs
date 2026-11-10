#![no_std]

extern crate alloc;

pub mod async_state;
pub mod completion_state;
pub mod deferred_state;
pub mod dma_state;
pub mod dns_state;
pub mod driver_registry;
pub mod drivers;
pub mod fw_cfg;
pub mod irq_platform;
pub mod mutex_state;
pub mod net_async_state;
pub mod net_stack_state;
pub mod net_state;
pub mod network_config;
pub mod pci_state;
pub mod run_queue;
#[cfg(feature = "guest-tests")]
pub mod test_cleanup;
pub mod test_selection;
pub mod udp_state;

#[cfg(test)]
extern crate std;
