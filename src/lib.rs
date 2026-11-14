#![no_std]

extern crate alloc;

#[path = "runtime/async_state.rs"]
pub mod async_state;
#[path = "runtime/completion_state.rs"]
pub mod completion_state;
#[path = "drivers/deferred_state.rs"]
pub mod deferred_state;
#[path = "memory/dma_state.rs"]
pub mod dma_state;
#[path = "net/dns_state.rs"]
pub mod dns_state;
#[path = "drivers/registry.rs"]
pub mod driver_registry;
#[path = "drivers/manager.rs"]
pub mod drivers;
#[path = "boot/fw_cfg.rs"]
pub mod fw_cfg;
#[path = "arch/x86_64/irq_platform.rs"]
pub mod irq_platform;
#[path = "linux_compat/abi.rs"]
pub mod linux_abi;
#[path = "runtime/mutex_state.rs"]
pub mod mutex_state;
#[path = "net/async_state.rs"]
pub mod net_async_state;
#[path = "net/stack_state.rs"]
pub mod net_stack_state;
#[path = "net/state.rs"]
pub mod net_state;
#[path = "net/config.rs"]
pub mod network_config;
#[path = "drivers/pci_state.rs"]
pub mod pci_state;
#[path = "runtime/run_queue.rs"]
pub mod run_queue;
#[path = "net/tcp_state.rs"]
pub mod tcp_state;
#[cfg(feature = "guest-tests")]
#[path = "testing/cleanup.rs"]
pub mod test_cleanup;
#[path = "testing/selection.rs"]
pub mod test_selection;
#[path = "net/udp_state.rs"]
pub mod udp_state;
#[path = "user/state.rs"]
pub mod user_state;

#[cfg(test)]
extern crate std;
