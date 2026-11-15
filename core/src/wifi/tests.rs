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

#[test]
fn note_link_loss_after_long_association_refreshes_watchdog() {
    const ELEVEN_HOURS_MS: u64 = 11 * 60 * 60 * 1000;

    let mut watchdog = NoIpWatchdog::new();
    watchdog.note_last_good(STABLE_LEASE_MS);
    watchdog.note_link_loss(0, ELEVEN_HOURS_MS);

    assert!(!watchdog.reboot_due(ELEVEN_HOURS_MS));
    assert!(!watchdog.reboot_due(ELEVEN_HOURS_MS + NO_IP_REBOOT_MS - 1));
    assert!(watchdog.reboot_due(ELEVEN_HOURS_MS + NO_IP_REBOOT_MS));
}

#[test]
fn note_link_loss_during_link_flapping_does_not_refresh_watchdog() {
    let mut watchdog = NoIpWatchdog::new();
    let mut now = 0;
    let flap_lease_ms = 10_000;
    assert!(flap_lease_ms < STABLE_LEASE_MS);

    // Repeated 10-second flapping leases (< STABLE_LEASE_MS) never refresh
    // the watchdog countdown, so the 10-minute reboot timer fires on schedule.
    while now < NO_IP_REBOOT_MS {
        let lease_start = now;
        now += flap_lease_ms;
        watchdog.note_link_loss(lease_start, now);
        now += 5_000;
    }

    assert!(watchdog.reboot_due(NO_IP_REBOOT_MS));
}

#[test]
fn repeated_mqtt_rejoins_do_not_refresh_no_ip_watchdog() {
    let mut watchdog = NoIpWatchdog::new();
    let mut now = 0;
    let rejoin_cycle_ms = 65_000;
    assert!(rejoin_cycle_ms < STABLE_LEASE_MS);

    // Each MQTT rejoin cycle takes ~65s (< STABLE_LEASE_MS = 90s).
    // Even if link loss occurred right around the rejoin window, it does not
    // reset the 10-minute reboot escalation.
    for _ in 0..10 {
        let lease_start = now;
        now += rejoin_cycle_ms;
        watchdog.note_link_loss(lease_start, now);
    }

    assert!(now >= NO_IP_REBOOT_MS);
    assert!(watchdog.reboot_due(now));
}

#[test]
fn mqtt_session_healthy_requires_inbound_publish_or_full_keepalive_cycle() {
    let connected_at = 1_000;

    // Immediately after connect: not healthy.
    assert!(!mqtt_session_healthy(connected_at, connected_at, 0, false));

    // After SUBACK (#1) before keepalive window: not healthy.
    assert!(!mqtt_session_healthy(
        connected_at,
        connected_at + 100,
        1,
        false
    ));

    // After PINGREQ flush (#2) at keepalive window: still not healthy until PINGRESP (#3).
    assert!(!mqtt_session_healthy(
        connected_at,
        connected_at + MQTT_HEALTHY_SESSION_MS,
        2,
        false
    ));

    // 3 idle polls before keepalive window: not healthy.
    assert!(!mqtt_session_healthy(
        connected_at,
        connected_at + MQTT_HEALTHY_SESSION_MS - 1,
        3,
        false
    ));

    // After PINGRESP (#3) at keepalive window: healthy.
    assert!(mqtt_session_healthy(
        connected_at,
        connected_at + MQTT_HEALTHY_SESSION_MS,
        3,
        false
    ));
    assert!(mqtt_session_healthy(
        connected_at,
        connected_at + MQTT_HEALTHY_SESSION_MS + 10_000,
        4,
        false
    ));

    // Inbound application publish marks the session healthy immediately.
    assert!(mqtt_session_healthy(connected_at, connected_at, 0, true));
}

#[test]
fn task_stalled_detects_missed_heartbeats() {
    assert!(!task_stalled(1_000, 1_000));
    assert!(!task_stalled(1_000, 1_000 + TASK_STALL_TIMEOUT_MS - 1));
    assert!(task_stalled(1_000, 1_000 + TASK_STALL_TIMEOUT_MS));
    assert!(task_stalled(1_000, 1_000 + TASK_STALL_TIMEOUT_MS * 2));
    // Backwards clock saturates to 0 and does not falsely trip.
    assert!(!task_stalled(10_000, 5_000));
}
