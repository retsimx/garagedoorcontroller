use super::*;

#[test]
fn trigger_accepted_from_idle() {
    let mut interlock = RelayInterlock::new();
    assert_eq!(interlock.trigger(0), Ok(RelayAction::AssertHigh));
    assert!(interlock.is_pulsing(0));
}

#[test]
fn pulse_holds_599ms_releases_600ms() {
    let mut interlock = RelayInterlock::new();
    assert_eq!(interlock.trigger(0), Ok(RelayAction::AssertHigh));

    assert!(interlock.is_pulsing(0));
    assert!(interlock.is_pulsing(599));
    assert!(!interlock.is_pulsing(600));

    assert_eq!(interlock.poll(0), None);
    assert_eq!(interlock.poll(599), None);
    assert_eq!(interlock.poll(600), Some(RelayAction::DeassertLow));
    assert!(!interlock.is_pulsing(600));
    assert_eq!(interlock.poll(600), None);
}

#[test]
fn trigger_rejected_during_active_pulse() {
    let mut interlock = RelayInterlock::new();
    assert_eq!(interlock.trigger(0), Ok(RelayAction::AssertHigh));
    assert_eq!(interlock.trigger(100), Err(TriggerRejection::PulseActive));
    assert_eq!(interlock.trigger(599), Err(TriggerRejection::PulseActive));
}

#[test]
fn trigger_rejected_during_cooldown() {
    let mut interlock = RelayInterlock::new();
    assert_eq!(interlock.trigger(0), Ok(RelayAction::AssertHigh));
    assert_eq!(interlock.poll(600), Some(RelayAction::DeassertLow));

    assert_eq!(
        interlock.trigger(600),
        Err(TriggerRejection::Cooldown { remaining_ms: 2000 })
    );
    assert_eq!(
        interlock.trigger(2599),
        Err(TriggerRejection::Cooldown { remaining_ms: 1 })
    );
    assert_eq!(interlock.trigger(2600), Ok(RelayAction::AssertHigh));
}

#[test]
fn rapid_triggers_produce_single_pulse() {
    let mut interlock = RelayInterlock::new();
    assert_eq!(interlock.trigger(0), Ok(RelayAction::AssertHigh));
    assert_eq!(interlock.trigger(100), Err(TriggerRejection::PulseActive));
    assert_eq!(interlock.trigger(200), Err(TriggerRejection::PulseActive));
    assert_eq!(interlock.poll(600), Some(RelayAction::DeassertLow));
    assert_eq!(interlock.poll(600), None);
}

#[test]
fn debounce_rejects_10ms_glitch() {
    let mut debouncer = ReedDebouncer::new(DoorState::Closed);
    assert_eq!(debouncer.update(false, 0), None);
    assert!(debouncer.is_pending());
    assert_eq!(debouncer.update(true, 10), None);
    assert!(!debouncer.is_pending());
    assert_eq!(debouncer.state(), DoorState::Closed);
}

#[test]
fn debounce_commits_after_55ms() {
    let mut debouncer = ReedDebouncer::new(DoorState::Closed);
    assert_eq!(debouncer.update(false, 0), None);
    assert_eq!(debouncer.update(false, 55), Some(DoorState::Open));
    assert_eq!(debouncer.state(), DoorState::Open);
    assert!(!debouncer.is_pending());
}

#[test]
fn debounce_boundary_49_reject_50_commit() {
    let mut debouncer = ReedDebouncer::new(DoorState::Closed);
    assert_eq!(debouncer.update(false, 0), None);
    assert_eq!(debouncer.update(false, 49), None);
    assert_eq!(debouncer.state(), DoorState::Closed);

    let mut debouncer = ReedDebouncer::new(DoorState::Closed);
    assert_eq!(debouncer.update(false, 0), None);
    assert_eq!(debouncer.update(false, 50), Some(DoorState::Open));
}

#[test]
fn debounce_commits_open_to_closed() {
    let mut debouncer = ReedDebouncer::new(DoorState::Open);
    assert_eq!(debouncer.update(true, 0), None);
    assert_eq!(debouncer.update(true, 55), Some(DoorState::Closed));
    assert_eq!(debouncer.state(), DoorState::Closed);
    assert!(!debouncer.is_pending());
}

#[test]
fn debounce_preserves_candidate_across_fast_samples() {
    // Sampling faster than the debounce interval must not reset the candidate
    // timer; the transition still commits once 50 ms of stability have elapsed.
    let mut debouncer = ReedDebouncer::new(DoorState::Closed);
    assert_eq!(debouncer.update(false, 0), None);
    assert_eq!(debouncer.update(false, 10), None);
    assert_eq!(debouncer.update(false, 20), None);
    assert_eq!(debouncer.update(false, 40), None);
    assert_eq!(debouncer.update(false, 50), Some(DoorState::Open));
    assert_eq!(debouncer.state(), DoorState::Open);
}

#[test]
fn cooldown_rearms_after_second_pulse() {
    let mut interlock = RelayInterlock::new();
    assert_eq!(interlock.trigger(0), Ok(RelayAction::AssertHigh));
    assert_eq!(interlock.poll(600), Some(RelayAction::DeassertLow));

    assert_eq!(interlock.trigger(2600), Ok(RelayAction::AssertHigh));
    assert_eq!(interlock.poll(3200), Some(RelayAction::DeassertLow));

    assert_eq!(
        interlock.trigger(3200),
        Err(TriggerRejection::Cooldown { remaining_ms: 2000 })
    );
    assert_eq!(
        interlock.trigger(5199),
        Err(TriggerRejection::Cooldown { remaining_ms: 1 })
    );
    assert_eq!(interlock.trigger(5200), Ok(RelayAction::AssertHigh));
}

#[test]
fn redundant_sample_emits_no_event() {
    let mut closed = ReedDebouncer::new(DoorState::Closed);
    assert_eq!(closed.update(true, 0), None);
    assert!(!closed.is_pending());
    assert_eq!(closed.update(true, 100), None);
    assert_eq!(closed.update(true, 1000), None);
    assert_eq!(closed.state(), DoorState::Closed);

    let mut open = ReedDebouncer::new(DoorState::Open);
    assert_eq!(open.update(false, 0), None);
    assert_eq!(open.update(false, 1000), None);
    assert_eq!(open.state(), DoorState::Open);
}
