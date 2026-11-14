pub mod async_state;
pub mod completion_state;
pub mod mutex_state;
pub mod run_queue;

#[cfg(target_os = "none")]
pub(crate) mod async_runtime;
#[cfg(target_os = "none")]
pub(crate) mod async_sync;
#[cfg(target_os = "none")]
pub(crate) mod completion;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional native runtime APIs are available to kernel clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod kernel_runtime;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod mutex;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod threads;
#[cfg(target_os = "none")]
pub(crate) mod time;
