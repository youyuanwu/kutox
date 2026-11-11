// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! Task lifecycle, saved contexts and executable startup primitives adapted from Moss.

pub mod async_state;
pub mod context;
pub mod elf;
pub mod user_stack;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Runnable,
    Running,
    Sleeping,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidTransition;

impl TaskState {
    pub fn activate(&mut self) -> Result<(), InvalidTransition> {
        self.transition(Self::Runnable, Self::Running)
    }

    pub fn mark_runnable(&mut self) -> Result<(), InvalidTransition> {
        self.transition(Self::Running, Self::Runnable)
    }

    pub fn finish(&mut self) -> Result<(), InvalidTransition> {
        self.transition(Self::Running, Self::Finished)
    }

    pub fn sleep(&mut self) -> Result<(), InvalidTransition> {
        self.transition(Self::Running, Self::Sleeping)
    }

    pub fn wake(&mut self) -> Result<(), InvalidTransition> {
        self.transition(Self::Sleeping, Self::Runnable)
    }

    fn transition(&mut self, from: Self, to: Self) -> Result<(), InvalidTransition> {
        if *self != from {
            return Err(InvalidTransition);
        }
        *self = to;
        Ok(())
    }
}
