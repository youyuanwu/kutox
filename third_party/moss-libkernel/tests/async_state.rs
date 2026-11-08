use moss_libkernel::task::{
    InvalidTransition, TaskState,
    async_state::{State, WakeAction},
};

#[test]
fn synchronous_transitions_are_unchanged() {
    let mut state = TaskState::Runnable;
    assert_eq!(state.sleep(), Err(InvalidTransition));
    state.activate().unwrap();
    state.sleep().unwrap();
    state.wake().unwrap();
    state.activate().unwrap();
    state.mark_runnable().unwrap();
    state.activate().unwrap();
    state.finish().unwrap();
    assert_eq!(state.wake(), Err(InvalidTransition));
    assert_eq!(state, TaskState::Finished);
}

#[test]
fn each_wake_boundary_preserves_the_next_poll() {
    for boundary in 0..4 {
        let mut state = State::Runnable;
        if boundary == 0 {
            assert_eq!(state.wake(), WakeAction::None);
        }
        state.activate().unwrap();
        if boundary == 1 {
            assert_eq!(state.wake(), WakeAction::PreventedSleep);
        }
        state.prepare_sleep().unwrap();
        if boundary == 2 {
            assert_eq!(state.wake(), WakeAction::PreventedSleep);
        }
        assert_eq!(state.finalize_sleep(), Ok(boundary == 0 || boundary == 3));
        if boundary == 3 {
            assert_eq!(state.wake(), WakeAction::Enqueue);
        }
        if boundary != 0 {
            assert_eq!(state, State::Runnable);
            state.activate().unwrap();
            state.finish().unwrap();
        } else {
            assert_eq!(state, State::Sleeping);
            state.cancel_idle().unwrap();
        }
        assert_eq!(state.wake(), WakeAction::None);
        assert_eq!(state, State::Finished);
    }
}

#[test]
fn repeated_wakes_coalesce_and_completion_wins() {
    let mut state = State::Runnable;
    for _ in 0..32 {
        assert_eq!(state.wake(), WakeAction::None);
    }
    state.activate().unwrap();
    assert_eq!(state.wake(), WakeAction::PreventedSleep);
    for _ in 0..32 {
        assert_eq!(state.wake(), WakeAction::None);
    }
    state.finish().unwrap();
    assert_eq!(state.wake(), WakeAction::None);
    assert_eq!(state, State::Finished);
}

#[test]
fn invalid_transitions_leave_all_states_unchanged() {
    for initial in [
        State::Runnable,
        State::Running,
        State::Woken,
        State::PendingSleep,
        State::Sleeping,
        State::Finished,
    ] {
        let mut state = initial;
        if initial != State::Runnable {
            assert_eq!(state.activate(), Err(InvalidTransition));
            assert_eq!(state, initial);
        }
        if !matches!(initial, State::Running | State::Woken) {
            assert_eq!(state.prepare_sleep(), Err(InvalidTransition));
            assert_eq!(state, initial);
        }
        if !matches!(initial, State::PendingSleep | State::Woken) {
            assert_eq!(state.finalize_sleep(), Err(InvalidTransition));
            assert_eq!(state, initial);
        }
        if !matches!(initial, State::Running | State::Woken | State::PendingSleep) {
            assert_eq!(state.finish(), Err(InvalidTransition));
            assert_eq!(state, initial);
        }
        if !matches!(initial, State::Runnable | State::Sleeping) {
            assert_eq!(state.cancel_idle(), Err(InvalidTransition));
            assert_eq!(state, initial);
        }
    }
}
