use super::*;
use std::{
    boxed::Box,
    collections::VecDeque,
    future::Future,
    marker::PhantomPinned,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
    vec::Vec,
};

fn sleep(state: &mut AsyncState, task: TaskHandle) {
    assert_eq!(state.begin_poll(), Ok(Some(task)));
    state.prepare_pending(task).unwrap();
    state.complete_pending(task).unwrap();
    assert_eq!(state.state(task), Ok(State::Sleeping));
}

fn finish(state: &mut AsyncState, task: TaskHandle) {
    assert_eq!(state.begin_poll(), Ok(Some(task)));
    state.complete_poll(task, Ok(())).unwrap();
    assert_eq!(state.release_payload(task), Ok(Outcome::Complete));
}

#[test]
fn exact_task_and_payload_bounds_are_atomic() {
    let mut state = AsyncState::new().unwrap();
    assert_eq!(
        state.admit(MAX_FUTURE_BYTES + 1),
        Err(Error::PayloadTooLarge)
    );
    assert_eq!(state.retained_records(), 0);
    let tasks: Vec<_> = (0..MAX_TASKS)
        .map(|_| state.admit(MAX_FUTURE_BYTES).unwrap())
        .collect();
    assert_eq!(state.admit(0), Err(Error::Capacity));
    assert_eq!(state.retained_records(), 16);
    assert_eq!(state.ready_len(), 16);
    for task in tasks {
        finish(&mut state, task);
    }
    assert_eq!(state.retained_records(), 0);
    assert_eq!(state.ready_len(), 0);
    assert_eq!(state.begin_poll(), Ok(None));
}

#[test]
fn metadata_is_bounded_separately_from_executor_owned_payloads() {
    let bytes = core::mem::size_of::<AsyncState>();
    std::println!("async policy metadata_bytes={bytes}");
    assert!(bytes <= 4096);
    assert_eq!(MAX_TASKS * MAX_FUTURE_BYTES, 32 * 1024);
}

#[test]
fn sixteen_distinct_waiters_are_notified_once_and_reclaim_registrations() {
    let mut state = AsyncState::new().unwrap();
    let tasks: Vec<_> = (0..MAX_TASKS).map(|_| state.admit(0).unwrap()).collect();
    let mut event = state.new_event().unwrap();
    for task in &tasks {
        assert_eq!(state.begin_poll(), Ok(Some(*task)));
        assert!(matches!(
            state.register_event(*task, &event, event.prepare()),
            Ok(EventWait::Registered(_))
        ));
        state.prepare_pending(*task).unwrap();
        state.complete_pending(*task).unwrap();
    }
    assert_eq!(state.registration_count(), 16);
    assert_eq!(state.ready_len(), 0);
    assert_eq!(state.signal(&mut event), Ok(16));
    assert_eq!(state.signal(&mut event), Ok(0));
    assert_eq!(state.ready_len(), 16);
    assert_eq!(state.registration_count(), 0);
    for task in tasks {
        finish(&mut state, task);
    }
}

#[test]
fn repeated_wakes_never_duplicate_fifo_entries() {
    let mut state = AsyncState::new().unwrap();
    let first = state.admit(0).unwrap();
    let second = state.admit(0).unwrap();
    let waker = state.retain_waker(first).unwrap();
    for _ in 0..32 {
        assert_eq!(state.wake(&waker), Ok(WakeAction::None));
    }
    assert_eq!(state.ready_len(), 2);
    assert_eq!(state.begin_poll(), Ok(Some(first)));
    assert_eq!(state.wake(&waker), Ok(WakeAction::PreventedSleep));
    for _ in 0..32 {
        assert_eq!(state.wake(&waker), Ok(WakeAction::None));
    }
    state.prepare_pending(first).unwrap();
    state.complete_pending(first).unwrap();
    finish(&mut state, second);
    finish(&mut state, first);
    assert_eq!(state.wake(&waker), Ok(WakeAction::None));
    assert_eq!(state.retained_records(), 1);
    state.release_waker(waker).unwrap();
    assert_eq!(state.retained_records(), 0);
}

#[test]
fn wake_during_poll_before_deactivation_or_after_sleep_is_not_lost() {
    for boundary in 0..3 {
        let mut state = AsyncState::new().unwrap();
        let task = state.admit(0).unwrap();
        let waker = state.retain_waker(task).unwrap();
        assert_eq!(state.begin_poll(), Ok(Some(task)));
        if boundary == 0 {
            assert_eq!(state.wake(&waker), Ok(WakeAction::PreventedSleep));
        }
        state.prepare_pending(task).unwrap();
        if boundary == 1 {
            assert_eq!(state.wake(&waker), Ok(WakeAction::PreventedSleep));
        }
        state.complete_pending(task).unwrap();
        if boundary == 2 {
            assert_eq!(state.wake(&waker), Ok(WakeAction::Enqueue));
        }
        assert_eq!(state.ready_len(), 1);
        finish(&mut state, task);
        state.release_waker(waker).unwrap();
    }
}

#[test]
fn event_prepare_register_recheck_covers_each_wake_boundary() {
    for boundary in 0..4 {
        let mut state = AsyncState::new().unwrap();
        let task = state.admit(0).unwrap();
        let mut event = state.new_event().unwrap();
        assert_eq!(state.begin_poll(), Ok(Some(task)));
        let token = event.prepare();
        if boundary == 0 {
            assert_eq!(state.signal(&mut event), Ok(0));
            assert_eq!(
                state.register_event(task, &event, token),
                Ok(EventWait::Changed)
            );
            state.complete_poll(task, Ok(())).unwrap();
        } else {
            let EventWait::Registered(id) = state.register_event(task, &event, token).unwrap()
            else {
                panic!("unchanged event must register");
            };
            if boundary == 1 {
                assert_eq!(state.signal(&mut event), Ok(1));
            }
            state.prepare_pending(task).unwrap();
            if boundary == 2 {
                assert_eq!(state.signal(&mut event), Ok(1));
            }
            state.complete_pending(task).unwrap();
            if boundary == 3 {
                assert_eq!(state.signal(&mut event), Ok(1));
            }
            assert!(!state.unregister_event(id));
            assert_eq!(state.registration_count(), 0);
            assert_eq!(state.begin_poll(), Ok(Some(task)));
            state.complete_poll(task, Ok(())).unwrap();
        }
        state.release_payload(task).unwrap();
    }
}

#[test]
fn registration_and_deadline_n_plus_one_admission_and_cancel_reclaim() {
    let mut state = AsyncState::new().unwrap();
    let task = state.admit(0).unwrap();
    let event = state.new_event().unwrap();
    assert_eq!(state.begin_poll(), Ok(Some(task)));
    let ids: Vec<_> = (0..MAX_REGISTRATIONS)
        .map(
            |_| match state.register_event(task, &event, event.prepare()).unwrap() {
                EventWait::Registered(id) => id,
                _ => panic!("event did not change"),
            },
        )
        .collect();
    assert_eq!(state.registration_count(), 16);
    assert_eq!(
        state.register_event(task, &event, event.prepare()),
        Err(Error::RegistrationCapacity)
    );
    let deadlines: Vec<_> = (0..MAX_DEADLINES)
        .map(|index| state.add_deadline(task, 100 + index as u64).unwrap())
        .collect();
    assert_eq!(state.add_deadline(task, 0), Err(Error::DeadlineCapacity));
    assert_eq!(state.deadline_count(), 16);
    assert_eq!(state.next_deadline(), Some(100));
    state.cancel(task).unwrap();
    assert_eq!(state.registration_count(), 0);
    assert_eq!(state.deadline_count(), 0);
    assert_eq!(state.release_payload(task), Err(Error::InvalidState));
    state.prepare_pending(task).unwrap();
    state.complete_pending(task).unwrap();
    assert_eq!(state.release_payload(task), Ok(Outcome::Cancelled));
    for id in ids {
        assert!(!state.unregister_event(id));
    }
    for id in deadlines {
        assert!(!state.remove_deadline(id));
    }
}

#[test]
fn unregister_removes_only_its_registration_and_reused_slots_have_fresh_ids() {
    let mut state = AsyncState::new().unwrap();
    let task = state.admit(0).unwrap();
    let mut event = state.new_event().unwrap();
    assert_eq!(state.begin_poll(), Ok(Some(task)));
    let register =
        |state: &mut AsyncState| match state.register_event(task, &event, event.prepare()).unwrap()
        {
            EventWait::Registered(id) => id,
            _ => panic!("event did not change"),
        };
    let old = register(&mut state);
    assert!(state.unregister_event(old));
    let replacement = register(&mut state);
    assert_ne!(old, replacement);
    assert!(!state.unregister_event(old));
    assert_eq!(state.signal(&mut event), Ok(1));
    assert!(!state.unregister_event(replacement));
    state.complete_poll(task, Ok(())).unwrap();
    state.release_payload(task).unwrap();
}

#[test]
fn deadline_expiry_is_one_shot_and_coalesces_multiple_due_wakes() {
    let mut state = AsyncState::new().unwrap();
    let first = state.admit(0).unwrap();
    let second = state.admit(0).unwrap();
    assert_eq!(state.begin_poll(), Ok(Some(first)));
    let removed = state.add_deadline(first, 9).unwrap();
    assert!(state.remove_deadline(removed));
    assert!(!state.remove_deadline(removed));
    state.add_deadline(first, 10).unwrap();
    state.add_deadline(first, 10).unwrap();
    state.prepare_pending(first).unwrap();
    state.complete_pending(first).unwrap();
    assert_eq!(state.begin_poll(), Ok(Some(second)));
    state.add_deadline(second, 20).unwrap();
    state.prepare_pending(second).unwrap();
    state.complete_pending(second).unwrap();
    assert_eq!(state.expire(9), Ok(0));
    assert_eq!(state.expire(10), Ok(2));
    assert_eq!(state.expire(10), Ok(0));
    assert_eq!(state.ready_len(), 1);
    assert_eq!(state.next_deadline(), Some(20));
    finish(&mut state, first);
    assert_eq!(state.expire(20), Ok(1));
    assert_eq!(state.next_deadline(), None);
    finish(&mut state, second);
    assert_eq!(deadline_after(u64::MAX, 1), Err(Error::DeadlineOverflow));
    assert_eq!(deadline_after(u64::MAX, 0), Ok(u64::MAX));
}

#[test]
fn retained_wakers_charge_the_same_pool_across_closed_epochs() {
    let mut state = AsyncState::new().unwrap();
    let mut tasks = Vec::new();
    let mut wakers = Vec::new();
    for _ in 0..MAX_TASKS {
        let task = state.admit(0).unwrap();
        tasks.push(task);
        wakers.push(state.retain_waker(task).unwrap());
    }
    let old_epoch = state.epoch();
    let old_event = state.new_event().unwrap();
    state.close().unwrap();
    assert_eq!(state.admit(0), Err(Error::Closed));
    assert_eq!(state.reopen(), Err(Error::Busy));
    for task in &tasks {
        assert_eq!(state.release_payload(*task), Ok(Outcome::Cancelled));
    }
    assert_eq!(state.retained_records(), 16);
    assert_ne!(state.reopen().unwrap(), old_epoch);
    assert_eq!(state.admit(0), Err(Error::Capacity));
    for waker in &wakers {
        assert_eq!(state.wake(waker), Ok(WakeAction::None));
    }
    let clone = state.clone_waker(&wakers[0]).unwrap();
    state.release_waker(wakers.remove(0)).unwrap();
    assert_eq!(state.admit(0), Err(Error::Capacity));
    state.release_waker(clone).unwrap();
    let replacement = state.admit(0).unwrap();
    assert_eq!(replacement.slot(), tasks[0].slot());
    assert_ne!(replacement, tasks[0]);
    assert_eq!(state.retain_waker(tasks[0]), Err(Error::Stale));
    assert_eq!(state.begin_poll(), Ok(Some(replacement)));
    assert_eq!(
        state.register_event(replacement, &old_event, old_event.prepare()),
        Err(Error::Stale)
    );
    for waker in &wakers {
        assert_eq!(state.wake(waker), Ok(WakeAction::None));
    }
    assert_eq!(state.ready_len(), 0);
    state.complete_poll(replacement, Ok(())).unwrap();
    state.release_payload(replacement).unwrap();
    for waker in wakers {
        state.release_waker(waker).unwrap();
    }
    assert_eq!(state.retained_records(), 0);
}

#[test]
fn cancellation_of_sleeping_ready_and_running_tasks_keeps_payload_ownership() {
    for running in [false, true] {
        let mut state = AsyncState::new().unwrap();
        let task = state.admit(0).unwrap();
        let waker = state.retain_waker(task).unwrap();
        if running {
            assert_eq!(state.begin_poll(), Ok(Some(task)));
        }
        state.cancel(task).unwrap();
        state.cancel(task).unwrap();
        assert_eq!(state.ready_len(), 0);
        assert_eq!(state.wake(&waker), Ok(WakeAction::None));
        if running {
            assert_eq!(state.release_payload(task), Err(Error::InvalidState));
            state
                .complete_poll(task, Err(TaskFailure::Service(-5)))
                .unwrap();
        }
        assert_eq!(state.retained_records(), 1);
        assert_eq!(state.release_payload(task), Ok(Outcome::Cancelled));
        assert_eq!(state.retained_records(), 1);
        assert_eq!(state.release_payload(task), Err(Error::InvalidState));
        state.release_waker(waker).unwrap();
        assert_eq!(state.retained_records(), 0);
    }
    let mut state = AsyncState::new().unwrap();
    let task = state.admit(0).unwrap();
    sleep(&mut state, task);
    state.cancel(task).unwrap();
    assert_eq!(state.release_payload(task), Ok(Outcome::Cancelled));
}

#[test]
fn closed_running_poll_must_return_before_reopen_or_payload_release() {
    let mut state = AsyncState::new().unwrap();
    let task = state.admit(0).unwrap();
    assert_eq!(state.begin_poll(), Ok(Some(task)));
    state.prepare_pending(task).unwrap();
    state.close().unwrap();
    state.close().unwrap();
    assert_eq!(state.release_payload(task), Err(Error::InvalidState));
    assert_eq!(state.reopen(), Err(Error::Busy));
    state.complete_pending(task).unwrap();
    assert_eq!(state.release_payload(task), Ok(Outcome::Cancelled));
    assert!(state.reopen().is_ok());
}

#[test]
fn failures_remain_failures_and_finished_wakes_do_not_queue() {
    for failure in [
        TaskFailure::NoMemory,
        TaskFailure::Context,
        TaskFailure::Service(-16),
    ] {
        let mut state = AsyncState::new().unwrap();
        let task = state.admit(0).unwrap();
        let waker = state.retain_waker(task).unwrap();
        assert_eq!(state.begin_poll(), Ok(Some(task)));
        state.complete_poll(task, Err(failure)).unwrap();
        assert_eq!(state.payload_released(task), Ok(false));
        assert_eq!(state.payload_count(), 1);
        assert_eq!(state.outcome(task), Ok(Some(Outcome::Failed(failure))));
        assert_eq!(state.wake(&waker), Ok(WakeAction::None));
        assert_eq!(state.ready_len(), 0);
        assert_eq!(state.release_payload(task), Ok(Outcome::Failed(failure)));
        state.release_waker(waker).unwrap();
    }
}

#[test]
fn published_cancellation_does_not_acknowledge_payload_retirement() {
    let mut state = AsyncState::new().unwrap();
    let task = state.admit(0).unwrap();
    let lease = state.retain_waker(task).unwrap();
    state.cancel(task).unwrap();
    assert_eq!(state.outcome(task), Ok(Some(Outcome::Cancelled)));
    assert_eq!(state.payload_released(task), Ok(false));
    state.release_payload(task).unwrap();
    assert_eq!(state.payload_count(), 0);
    assert_eq!(state.payload_released(task), Ok(true));
    state.release_waker(lease).unwrap();
    assert_eq!(state.payload_released(task), Err(Error::Stale));
    assert_eq!(
        crate::net_state::Error::from_code(-59),
        crate::net_state::Error::Context
    );
}

#[test]
fn task_identity_and_event_tokens_cannot_cross_tables_or_slot_reuse() {
    let mut first = AsyncState::new().unwrap();
    let mut second = AsyncState::new().unwrap();
    let old = first.admit(0).unwrap();
    let other = second.admit(0).unwrap();
    assert_eq!(second.state(old), Err(Error::Stale));
    assert_ne!(old.epoch(), other.epoch());
    assert_eq!(first.complete_poll(old, Ok(())), Err(Error::InvalidState));
    finish(&mut first, old);
    let replacement = first.admit(0).unwrap();
    assert_eq!(old.slot(), replacement.slot());
    assert_eq!(first.state(old), Err(Error::Stale));
    assert_eq!(first.begin_poll(), Ok(Some(replacement)));
    assert_eq!(first.begin_poll(), Err(Error::Busy));
    assert_eq!(
        first.complete_pending(replacement),
        Err(Error::InvalidState)
    );
    let event = first.new_event().unwrap();
    let wrong_event = first.new_event().unwrap();
    assert_eq!(
        first.register_event(replacement, &event, wrong_event.prepare()),
        Err(Error::Stale)
    );
    first.complete_poll(replacement, Ok(())).unwrap();
    first.release_payload(replacement).unwrap();
}

#[test]
fn checked_id_and_reference_exhaustion_are_nonmutating() {
    let source = AtomicU64::new(u64::MAX - 1);
    assert_eq!(allocate_id(&source), Ok(u64::MAX - 1));
    for _ in 0..3 {
        assert_eq!(allocate_id(&source), Err(Error::IdExhausted));
        assert_eq!(source.load(Ordering::Relaxed), u64::MAX);
    }
    let mut state = AsyncState::new().unwrap();
    let task = state.admit(0).unwrap();
    state.records[task.slot].as_mut().unwrap().wakers = usize::MAX;
    assert_eq!(state.retain_waker(task), Err(Error::ReferenceOverflow));
    assert_eq!(state.record(task).unwrap().wakers, usize::MAX);
    let mut event = state.new_event().unwrap();
    event.generation = u64::MAX;
    assert_eq!(state.signal(&mut event), Err(Error::GenerationExhausted));
    assert_eq!(event.prepare().generation, u64::MAX);
}

#[test]
fn self_waking_tasks_follow_fifo_instead_of_starving_peers() {
    let mut state = AsyncState::new().unwrap();
    let tasks: Vec<_> = (0..MAX_TASKS).map(|_| state.admit(0).unwrap()).collect();
    let wakers: Vec<_> = tasks
        .iter()
        .map(|task| state.retain_waker(*task).unwrap())
        .collect();
    for _ in 0..4 {
        for (task, waker) in tasks.iter().zip(&wakers) {
            assert_eq!(state.begin_poll(), Ok(Some(*task)));
            state.wake(waker).unwrap();
            state.prepare_pending(*task).unwrap();
            state.complete_pending(*task).unwrap();
        }
        assert_eq!(state.ready_len(), MAX_POLLS_PER_TURN);
    }
    state.close().unwrap();
    for task in tasks {
        state.release_payload(task).unwrap();
    }
    for waker in wakers {
        state.release_waker(waker).unwrap();
    }
}

#[test]
fn deterministic_wake_schedule_matches_a_reference_fifo() {
    let mut state = AsyncState::new().unwrap();
    let tasks: Vec<_> = (0..MAX_TASKS).map(|_| state.admit(0).unwrap()).collect();
    let wakers: Vec<_> = tasks
        .iter()
        .map(|task| state.retain_waker(*task).unwrap())
        .collect();
    let mut ready: VecDeque<_> = (0..MAX_TASKS).collect();
    let mut sleeping = [false; MAX_TASKS];
    let mut random = 0x5eed_u32;
    for _ in 0..10_000 {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let index = random as usize % MAX_TASKS;
        if random & 0x100 != 0 {
            if sleeping[index] {
                sleeping[index] = false;
                ready.push_back(index);
                assert_eq!(state.wake(&wakers[index]), Ok(WakeAction::Enqueue));
            } else {
                assert_eq!(state.wake(&wakers[index]), Ok(WakeAction::None));
            }
        } else if let Some(index) = ready.pop_front() {
            assert_eq!(state.begin_poll(), Ok(Some(tasks[index])));
            if random & 0x200 != 0 {
                state.wake(&wakers[index]).unwrap();
            }
            state.prepare_pending(tasks[index]).unwrap();
            if random & 0x200 != 0 {
                ready.push_back(index);
            } else {
                sleeping[index] = true;
            }
            state.complete_pending(tasks[index]).unwrap();
        } else {
            assert_eq!(state.begin_poll(), Ok(None));
        }
        assert_eq!(state.ready_len(), ready.len());
        for (index, asleep) in sleeping.iter().enumerate() {
            assert_eq!(
                state.state(tasks[index]),
                Ok(if *asleep {
                    State::Sleeping
                } else {
                    State::Runnable
                })
            );
        }
    }
    state.close().unwrap();
    for task in tasks {
        state.release_payload(task).unwrap();
    }
    for waker in wakers {
        state.release_waker(waker).unwrap();
    }
}

#[test]
fn pinned_future_stays_executor_owned_and_is_destroyed_outside_metadata_lock() {
    struct Payload {
        metadata: Arc<Mutex<AsyncState>>,
        dropped: Arc<Mutex<bool>>,
        _pin: PhantomPinned,
    }
    impl Future for Payload {
        type Output = ();
        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
            assert!(self.metadata.try_lock().is_ok());
            Poll::Pending
        }
    }
    impl Drop for Payload {
        fn drop(&mut self) {
            assert!(self.metadata.try_lock().is_ok());
            *self.dropped.lock().unwrap() = true;
        }
    }
    let metadata = Arc::new(Mutex::new(AsyncState::new().unwrap()));
    let dropped = Arc::new(Mutex::new(false));
    let mut payload = Box::pin(Payload {
        metadata: metadata.clone(),
        dropped: dropped.clone(),
        _pin: PhantomPinned,
    });
    let address = &*payload as *const Payload;
    let task = metadata
        .lock()
        .unwrap()
        .admit(core::mem::size_of::<Payload>())
        .unwrap();
    let waker = metadata.lock().unwrap().retain_waker(task).unwrap();
    for _ in 0..4 {
        assert_eq!(metadata.lock().unwrap().begin_poll(), Ok(Some(task)));
        assert_eq!(
            payload
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
        assert_eq!(&*payload as *const Payload, address);
        metadata.lock().unwrap().prepare_pending(task).unwrap();
        metadata.lock().unwrap().wake(&waker).unwrap();
        metadata.lock().unwrap().complete_pending(task).unwrap();
    }
    metadata.lock().unwrap().close().unwrap();
    assert!(!*dropped.lock().unwrap());
    drop(payload);
    assert!(*dropped.lock().unwrap());
    assert_eq!(
        metadata.lock().unwrap().release_payload(task),
        Ok(Outcome::Cancelled)
    );
    assert_eq!(metadata.lock().unwrap().retained_records(), 1);
    assert_eq!(metadata.lock().unwrap().wake(&waker), Ok(WakeAction::None));
    metadata.lock().unwrap().release_waker(waker).unwrap();
    assert_eq!(metadata.lock().unwrap().retained_records(), 0);
}
