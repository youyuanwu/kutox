pub(crate) mod cpu;
pub(crate) mod exceptions;
pub(crate) mod io;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
pub(crate) mod irq;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
pub(crate) mod platform;
pub(crate) mod serial;
