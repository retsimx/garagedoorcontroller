use super::*;

#[test]
fn no_ip_watchdog_is_due_after_threshold_without_a_lease() {
    let watchdog = NoIpWatchdog::new();
    assert!(!watchdog.reboot_due(0));
    assert!(!watchdog.reboot_due(NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(NO_IP_REBOOT_MS));
    assert!(watchdog.reboot_due(NO_IP_REBOOT_MS * 3));
}

#[test]
fn no_ip_watchdog_resets_on_lease() {
    let mut watchdog = NoIpWatchdog::new();
    watchdog.note_last_good(1_000);
    assert!(!watchdog.reboot_due(1_000));
    assert!(!watchdog.reboot_due(1_000 + NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(1_000 + NO_IP_REBOOT_MS));

    // A later lease moves the deadline.
    watchdog.note_last_good(500_000);
    assert!(!watchdog.reboot_due(500_000 + NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(500_000 + NO_IP_REBOOT_MS));
}

#[test]
fn no_ip_watchdog_handles_a_backwards_clock() {
    let mut watchdog = NoIpWatchdog::new();
    watchdog.note_last_good(10_000);
    assert!(!watchdog.reboot_due(5_000));
}

#[test]
fn link_loss_after_a_long_association_does_not_reboot() {
    const ELEVEN_HOURS_MS: u64 = 11 * 60 * 60 * 1000;

    let mut watchdog = NoIpWatchdog::new();
    watchdog.note_last_good(0);

    // The trap: the lease was stamped at association, so a healthy 11 hour
    // association already looks overdue for a reset the moment the link drops.
    assert!(watchdog.reboot_due(ELEVEN_HOURS_MS));

    // The fix: the countdown restarts at the link loss, not the association.
    watchdog.note_last_good(ELEVEN_HOURS_MS);
    assert!(!watchdog.reboot_due(ELEVEN_HOURS_MS + NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(ELEVEN_HOURS_MS + NO_IP_REBOOT_MS));
}

#[test]
fn rejoin_counter_trips_exactly_at_the_threshold() {
    let mut counter = RejoinCounter::new();
    for _ in 0..MQTT_REJOIN_FAILURES - 1 {
        assert!(!counter.failure());
    }
    assert!(counter.failure());
    assert_eq!(counter.failures(), 0, "counter resets after tripping");
}

#[test]
fn rejoin_counter_resets_on_success() {
    let mut counter = RejoinCounter::new();
    for _ in 0..MQTT_REJOIN_FAILURES - 1 {
        assert!(!counter.failure());
    }
    counter.success();
    assert_eq!(counter.failures(), 0);
    for _ in 0..MQTT_REJOIN_FAILURES - 1 {
        assert!(!counter.failure());
    }
    assert!(counter.failure());
}

#[test]
fn rejoin_counter_keeps_tripping_on_long_failure_runs() {
    let mut counter = RejoinCounter::new();
    let mut trips = 0;
    for _ in 0..MQTT_REJOIN_FAILURES * 2 {
        if counter.failure() {
            trips += 1;
        }
    }
    assert_eq!(trips, 2);
}
