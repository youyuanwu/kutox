// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Moss's wake/pending-sleep protocol under caller-serialized mutable access.

use super::InvalidTransition;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Runnable,
    Running,
    Woken,
    PendingSleep,
    Sleeping,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WakeAction {
    Enqueue,
    PreventedSleep,
    None,
}

impl State {
    pub fn activate(&mut self) -> Result<(), InvalidTransition> {
        if *self != Self::Runnable {
            return Err(InvalidTransition);
        }
        *self = Self::Running;
        Ok(())
    }

    pub fn prepare_sleep(&mut self) -> Result<(), InvalidTransition> {
        match self {
            Self::Running => *self = Self::PendingSleep,
            Self::Woken => {}
            _ => return Err(InvalidTransition),
        }
        Ok(())
    }

    /// Returns true only when the dispatcher may stop polling this task.
    pub fn finalize_sleep(&mut self) -> Result<bool, InvalidTransition> {
        match self {
            Self::PendingSleep => {
                *self = Self::Sleeping;
                Ok(true)
            }
            Self::Woken => {
                *self = Self::Runnable;
                Ok(false)
            }
            _ => Err(InvalidTransition),
        }
    }

    pub fn wake(&mut self) -> WakeAction {
        match self {
            Self::Sleeping => {
                *self = Self::Runnable;
                WakeAction::Enqueue
            }
            Self::Running | Self::PendingSleep => {
                *self = Self::Woken;
                WakeAction::PreventedSleep
            }
            Self::Runnable | Self::Woken | Self::Finished => WakeAction::None,
        }
    }

    pub fn finish(&mut self) -> Result<(), InvalidTransition> {
        match self {
            Self::Running | Self::Woken | Self::PendingSleep => {
                *self = Self::Finished;
                Ok(())
            }
            _ => Err(InvalidTransition),
        }
    }

    pub fn cancel_idle(&mut self) -> Result<(), InvalidTransition> {
        match self {
            Self::Runnable | Self::Sleeping => {
                *self = Self::Finished;
                Ok(())
            }
            _ => Err(InvalidTransition),
        }
    }
}
