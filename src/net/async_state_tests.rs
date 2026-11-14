use super::*;

fn session(source: u64) -> SessionKey {
    SessionGate::new().claim(source, source + 10, 2).unwrap()
}

#[test]
fn exclusive_session_is_checked_revocable_and_does_not_alias_replacement() {
    let mut gate = SessionGate::new();
    assert_eq!(gate.claim(0, 1, 2), Err(Error::Stale));
    let old = gate.claim(1, 11, 2).unwrap();
    assert_eq!(gate.access(1, 2), Ok(()));
    assert_eq!(gate.access(1, 3), Err(Error::Busy));
    assert_eq!(gate.claim(1, 11, 2), Err(Error::Busy));
    gate.close_source(1);
    assert_eq!(gate.access(1, 2), Err(Error::Closed));
    assert!(!gate.is_open(old));
    gate.release(old).unwrap();
    let new = gate.claim(2, 12, 2).unwrap();
    assert_ne!(old, new);
    assert_eq!(gate.release(old), Err(Error::Stale));
    assert!(gate.is_open(new));
}

#[test]
fn exact_sixteen_packets_and_checked_pool_reuse() {
    let mut pool = PacketPool::new();
    for slot in 0..MAX_PACKETS {
        pool.enable(slot);
    }
    let packets: std::vec::Vec<_> = (0..16).map(|_| pool.claim().unwrap()).collect();
    assert_eq!(pool.claim(), Err(Error::Busy));
    assert_eq!(pool.used(), 16);
    pool.release(packets[0]).unwrap();
    let replacement = pool.claim().unwrap();
    assert_ne!(replacement, packets[0]);
    assert_eq!(pool.release(packets[0]), Err(Error::Stale));
    assert_eq!(pool.used(), 16);
}

#[test]
fn four_requests_include_completed_and_closed_consumer_records() {
    let mut requests = Requests::new();
    let old = session(1);
    let ids: std::vec::Vec<_> = (0..4).map(|_| requests.admit(old).unwrap()).collect();
    assert_eq!(requests.admit(old), Err(Error::Busy));
    requests.close_queued(old);
    assert_eq!(requests.count(), 4);
    assert_eq!(requests.admit(session(2)), Err(Error::Busy));
    requests.abandon(ids[0]).unwrap();
    let new = requests.admit(session(2)).unwrap();
    assert_ne!(ids[0], new);
    assert_eq!(requests.phase(ids[0]), Err(Error::Stale));
    assert_eq!(requests.abandon(ids[0]), Err(Error::Stale));
}

#[test]
fn cancellation_at_each_boundary_has_one_retirement_path() {
    let key = session(1);
    for boundary in 0..5 {
        let mut requests = Requests::new();
        let mut pool = PacketPool::new();
        pool.enable(0);
        let packet = pool.claim().unwrap();
        let id = requests.admit(key).unwrap();
        if boundary == 0 {
            requests.abandon(id).unwrap();
            assert_eq!(requests.count(), 0);
            pool.release(packet).unwrap();
            continue;
        }
        requests.queue(id, packet).unwrap();
        if boundary == 1 {
            requests.abandon(id).unwrap();
            assert_eq!(requests.orphan_packet(), Some((id, packet)));
            pool.release(packet).unwrap();
            requests.finish(id, Err(Error::Closed)).unwrap();
            assert_eq!(requests.count(), 0);
            continue;
        }
        assert_eq!(requests.next(key), Some((id, packet)));
        if boundary == 2 {
            requests.abandon(id).unwrap();
        }
        let ticket = TxTicket::new(1, 1).unwrap();
        requests.submitted(id, ticket).unwrap();
        pool.release(packet).unwrap();
        if boundary == 3 {
            requests.abandon(id).unwrap();
        }
        assert!(requests.source_users(key));
        requests.finishing(id).unwrap();
        requests.finish(id, Ok(60)).unwrap();
        if boundary == 4 {
            assert_eq!(requests.phase(id), Ok(Phase::Done(Ok(60))));
            requests.abandon(id).unwrap();
        }
        assert_eq!(requests.count(), 0);
        assert_eq!(requests.finish(id, Ok(60)), Err(Error::Stale));
        assert_eq!(pool.used(), 0);
    }
}

#[test]
fn closed_is_not_retirement_for_a_live_source_receipt() {
    for finishing in [false, true] {
        let key = session(1);
        let mut requests = Requests::new();
        let mut pool = PacketPool::new();
        pool.enable(0);
        let packet = pool.claim().unwrap();
        let id = requests.admit(key).unwrap();
        requests.queue(id, packet).unwrap();
        requests.next(key).unwrap();
        let ticket = TxTicket::new(1, 1).unwrap();
        requests.submitted(id, ticket).unwrap();
        if finishing {
            requests.finishing(id).unwrap();
        }
        requests.close_queued(key);
        requests.abandon(id).unwrap();
        assert_eq!(
            requests.phase(id),
            Ok(if finishing {
                Phase::Finishing(ticket)
            } else {
                Phase::InFlight(ticket)
            })
        );
        assert!(requests.source_users(key));
        requests.source_retired(session(2));
        assert!(requests.source_users(key));
        requests.source_retired(key);
        assert_eq!(requests.count(), 0);
    }
}

#[test]
fn fifo_wrap_backpressure_and_one_source_tx_credit() {
    let key = session(1);
    let mut pool = PacketPool::new();
    for slot in 0..MAX_PACKETS {
        pool.enable(slot);
    }
    let packets: std::vec::Vec<_> = (0..12).map(|_| pool.claim().unwrap()).collect();
    let mut rx = RxQueue::new();
    for packet in &packets[..8] {
        rx.push(*packet).unwrap();
    }
    assert_eq!(rx.push(packets[8]), Err(Error::Busy));
    for expected in &packets[..4] {
        assert_eq!(rx.pop(), Some(*expected));
    }
    for packet in &packets[8..] {
        rx.push(*packet).unwrap();
    }
    for expected in &packets[4..] {
        assert_eq!(rx.pop(), Some(*expected));
    }
    assert_eq!(rx.pop(), None);
    let mut requests = Requests::new();
    let first = requests.admit(key).unwrap();
    let second = requests.admit(key).unwrap();
    requests.queue(first, packets[0]).unwrap();
    requests.queue(second, packets[1]).unwrap();
    assert_eq!(requests.next(key), Some((first, packets[0])));
    assert_eq!(requests.next(key), None);
    requests.retry(first, packets[0]).unwrap();
    assert_eq!(requests.next(key), Some((first, packets[0])));
}

#[test]
fn identity_exhaustion_is_nonwrapping() {
    let ids = AtomicU64::new(u64::MAX - 1);
    assert_eq!(identity(&ids), Ok(u64::MAX - 1));
    assert_eq!(identity(&ids), Err(Error::Driver(-240)));
    assert_eq!(ids.load(Ordering::Relaxed), u64::MAX);
}

#[test]
fn mixed_request_lifetimes_match_an_independent_ordered_model() {
    #[derive(Clone)]
    struct Model {
        id: RequestId,
        key: SessionKey,
        phase: Phase,
        consumer: bool,
        closing: bool,
        loan: Option<PacketId>,
    }
    let keys = [session(101), session(102)];
    let mut requests = Requests::new();
    let mut pool = PacketPool::new();
    for slot in 0..MAX_PACKETS {
        pool.enable(slot);
    }
    let mut model: std::vec::Vec<Model> = std::vec::Vec::new();
    let mut stale = std::vec::Vec::new();
    let mut random = 0x5eed1234_u32;
    let mut transitions = [0; 12];
    let mut phases = 0;
    for iteration in 0..30_000 {
        random = random.wrapping_mul(1664525).wrapping_add(1013904223);
        let operation = (random >> 16) as usize % 12;
        let key = keys[(random >> 8) as usize % 2];
        transitions[operation] += 1;
        if operation == 0 {
            if model.len() == MAX_REQUESTS {
                assert_eq!(requests.admit(key), Err(Error::Busy));
            } else {
                let id = requests.admit(key).unwrap();
                model.push(Model {
                    id,
                    key,
                    phase: Phase::Reserved,
                    consumer: true,
                    closing: false,
                    loan: Some(pool.claim().unwrap()),
                });
            }
        } else if operation == 1 {
            let busy = model.iter().any(|entry| {
                matches!(
                    entry.phase,
                    Phase::Calling(_) | Phase::InFlight(_) | Phase::Finishing(_)
                )
            });
            let next = if busy {
                None
            } else {
                model.iter().position(|entry| {
                    entry.key == key
                        && entry.consumer
                        && !entry.closing
                        && matches!(entry.phase, Phase::Queued(_))
                })
            };
            let expected = next.map(|index| {
                let Phase::Queued(packet) = model[index].phase else {
                    unreachable!()
                };
                model[index].phase = Phase::Calling(packet);
                (model[index].id, packet)
            });
            assert_eq!(requests.next(key), expected);
        } else if operation == 2 {
            requests.close_queued(key);
            for entry in &mut model {
                if entry.key == key {
                    match entry.phase {
                        Phase::Reserved => entry.phase = Phase::Done(Err(Error::Closed)),
                        Phase::Queued(_) => entry.closing = true,
                        _ => (),
                    }
                }
            }
        } else if operation == 3 {
            requests.source_retired(key);
            for entry in &mut model {
                if entry.key == key
                    && matches!(entry.phase, Phase::InFlight(_) | Phase::Finishing(_))
                {
                    entry.phase = Phase::Done(Err(Error::Closed));
                }
            }
        } else if operation == 4 {
            let orphan = requests.orphan_packet();
            if let Some((id, packet)) = orphan {
                let entry = model.iter_mut().find(|entry| entry.id == id).unwrap();
                assert_eq!(entry.phase, Phase::Queued(packet));
                assert!(!entry.consumer || entry.closing);
                requests.finish(id, Err(Error::Closed)).unwrap();
                entry.phase = Phase::Done(Err(Error::Closed));
                pool.release(entry.loan.take().unwrap()).unwrap();
            } else {
                assert!(
                    !model
                        .iter()
                        .any(|entry| matches!(entry.phase, Phase::Queued(_))
                            && (!entry.consumer || entry.closing))
                );
            }
        } else if !model.is_empty() {
            let index = (random >> 4) as usize % model.len();
            let entry = &mut model[index];
            match operation {
                5 => {
                    requests.abandon(entry.id).unwrap();
                    entry.consumer = false;
                }
                6 => {
                    if let Some(packet) = entry.loan {
                        let expected = if entry.phase == Phase::Reserved {
                            entry.phase = Phase::Queued(packet);
                            Ok(())
                        } else {
                            Err(Error::Busy)
                        };
                        assert_eq!(requests.queue(entry.id, packet), expected);
                    }
                }
                7 => {
                    let ticket = TxTicket::new(key.source(), iteration + 1).unwrap();
                    let expected = if matches!(entry.phase, Phase::Calling(_)) {
                        entry.phase = Phase::InFlight(ticket);
                        pool.release(entry.loan.take().unwrap()).unwrap();
                        Ok(())
                    } else {
                        Err(Error::Busy)
                    };
                    assert_eq!(requests.submitted(entry.id, ticket), expected);
                }
                8 => {
                    if let Some(packet) = entry.loan {
                        let expected = if entry.phase == Phase::Calling(packet) {
                            entry.phase = Phase::Queued(packet);
                            Ok(())
                        } else {
                            Err(Error::Busy)
                        };
                        assert_eq!(requests.retry(entry.id, packet), expected);
                    }
                }
                9 => {
                    let expected = if let Phase::InFlight(ticket) = entry.phase {
                        entry.phase = Phase::Finishing(ticket);
                        Ok(())
                    } else {
                        Err(Error::Busy)
                    };
                    assert_eq!(requests.finishing(entry.id), expected);
                }
                10 => {
                    let expected = if matches!(entry.phase, Phase::Done(_)) {
                        Err(Error::Stale)
                    } else {
                        entry.phase = Phase::Done(Ok(60));
                        if let Some(packet) = entry.loan.take() {
                            pool.release(packet).unwrap();
                        }
                        Ok(())
                    };
                    assert_eq!(requests.finish(entry.id, Ok(60)), expected);
                }
                11 => {
                    if let Some(id) = stale.last() {
                        assert_eq!(requests.abandon(*id), Err(Error::Stale));
                    }
                }
                _ => unreachable!(),
            }
        }
        model.retain_mut(|entry| {
            if !entry.consumer && matches!(entry.phase, Phase::Reserved | Phase::Done(_)) {
                if let Some(packet) = entry.loan.take() {
                    pool.release(packet).unwrap();
                }
                stale.push(entry.id);
                false
            } else {
                true
            }
        });
        assert_eq!(requests.count(), model.len());
        assert_eq!(
            pool.used(),
            model.iter().filter(|entry| entry.loan.is_some()).count()
        );
        for key in keys {
            assert_eq!(
                requests.source_users(key),
                model.iter().any(|entry| {
                    entry.key == key && !matches!(entry.phase, Phase::Reserved | Phase::Done(_))
                })
            );
        }
        for entry in &model {
            assert_eq!(requests.phase(entry.id), Ok(entry.phase));
            assert_eq!(requests.consumer(entry.id), Ok(entry.consumer));
            phases |= 1
                << match entry.phase {
                    Phase::Reserved => 0,
                    Phase::Queued(_) => 1,
                    Phase::Calling(_) => 2,
                    Phase::InFlight(_) => 3,
                    Phase::Finishing(_) => 4,
                    Phase::Done(_) => 5,
                };
        }
    }
    assert!(transitions.iter().all(|count| *count > 1000));
    assert_eq!(phases, 0b11_1111);
    assert!(stale.len() > 100);
    for entry in model {
        requests.abandon(entry.id).unwrap();
        let expected = if matches!(entry.phase, Phase::Reserved | Phase::Done(_)) {
            Err(Error::Stale)
        } else {
            Ok(())
        };
        assert_eq!(requests.finish(entry.id, Err(Error::Closed)), expected);
        if let Some(packet) = entry.loan {
            pool.release(packet).unwrap();
        }
    }
    assert_eq!(requests.count(), 0);
    assert_eq!(pool.used(), 0);
}
