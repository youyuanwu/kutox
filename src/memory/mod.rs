pub mod dma_state;

#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod dma;
#[cfg(target_os = "none")]
pub(crate) mod heap;

#[cfg(target_os = "none")]
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional kernel service APIs are available to native clients"
    )
)]
pub(crate) mod paging;
