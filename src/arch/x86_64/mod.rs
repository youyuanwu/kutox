pub mod irq_platform;

#[cfg(target_os = "none")]
pub(crate) mod cpu;
#[cfg(target_os = "none")]
pub(crate) mod exceptions;
#[cfg(target_os = "none")]
pub(crate) mod io;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod irq;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod platform;
#[cfg(target_os = "none")]
pub(crate) mod serial;
