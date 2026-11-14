pub(crate) mod async_runtime;
pub(crate) mod async_sync;
pub(crate) mod completion;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional native runtime APIs are available to kernel clients"
    )
)]
pub(crate) mod kernel_runtime;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
pub(crate) mod mutex;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
pub(crate) mod threads;
pub(crate) mod time;
