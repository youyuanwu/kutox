// SPDX-License-Identifier: MIT
// Copyright 2025 Matthew Leach
//! User-context ownership from Moss src/process/ctx.rs, without kernel/signal work.

pub struct Context<U> {
    user: U,
}

impl<U: Copy> Context<U> {
    pub fn from_user_ctx(user: U) -> Self {
        Self { user }
    }

    pub fn user(&self) -> &U {
        &self.user
    }

    pub fn save_user_ctx(&mut self, context: &U) {
        self.user = *context;
    }

    pub fn restore_user_ctx(&self) -> U {
        self.user
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saved_context_is_owned_and_restored_without_aliasing_the_trap_frame() {
        let mut frame = [1u64, 2, 3];
        let mut context = Context::from_user_ctx(frame);
        frame[0] = 4;
        assert_eq!(*context.user(), [1, 2, 3]);
        context.save_user_ctx(&frame);
        frame.fill(0);
        assert_eq!(frame, [0; 3]);
        frame = context.restore_user_ctx();
        assert_eq!(frame, [4, 2, 3]);
        frame[1] = 9;
        assert_eq!(frame, [4, 9, 3]);
        assert_eq!(*context.user(), [4, 2, 3]);
    }
}
