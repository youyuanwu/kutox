pub mod async_state;
pub mod config;
pub mod dns_state;
pub mod stack_state;
pub mod state;
pub mod tcp_state;
pub mod udp_state;

#[cfg(target_os = "none")]
pub(crate) mod async_service;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Additional native packet APIs are available to kernel clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod device;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(dead_code, reason = "DNS API is available to native kernel clients")
)]
#[cfg(target_os = "none")]
pub(crate) mod dns;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Native stack diagnostics are available to kernel clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod stack;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "TCP sockets are opened explicitly by native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod tcp;
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "Native UDP sockets are opened explicitly by kernel clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod udp;
