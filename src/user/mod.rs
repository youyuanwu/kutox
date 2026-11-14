pub mod state;

#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "User tasks are admitted explicitly by native clients"
    )
)]
#[cfg(target_os = "none")]
pub(crate) mod tasks;

#[cfg(target_os = "none")]
#[cfg_attr(
    not(feature = "guest-tests"),
    allow(
        dead_code,
        reason = "User programs are started explicitly by kernel clients"
    )
)]
pub(crate) mod execute;
