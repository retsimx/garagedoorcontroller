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
    watchdog.note_ip(1_000);
    assert!(!watchdog.reboot_due(1_000));
    assert!(!watchdog.reboot_due(1_000 + NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(1_000 + NO_IP_REBOOT_MS));

    // A later lease moves the deadline.
    watchdog.note_ip(500_000);
    assert!(!watchdog.reboot_due(500_000 + NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(500_000 + NO_IP_REBOOT_MS));
}

#[test]
fn no_ip_watchdog_handles_a_backwards_clock() {
    let mut watchdog = NoIpWatchdog::new();
    watchdog.note_ip(10_000);
    assert!(!watchdog.reboot_due(5_000));
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
