//! Pure Wi-Fi recovery policy: timing thresholds and the small decision
//! helpers the firmware's network tasks need.
//!
//! Kept free of embassy/cyw43 types so the decision boundaries can be unit
//! tested on the host, in the same spirit as [`crate::door`].

/// Maximum time to wait for CYW43 controller initialization (`control.init(clm)`
/// + `set_power_management`) at boot before resetting the MCU.
pub const RADIO_INIT_TIMEOUT_MS: u64 = 10_000;

/// Maximum time to wait for a CYW43 GPIO IOCTL (LED beacon) including mutex
/// acquisition so a wedged radio cannot deadlock `ControlMutex`.
pub const GPIO_TIMEOUT_MS: u64 = 1_000;

/// Maximum time to wait for one join attempt before treating it as failed.
///
/// The CYW43 firmware blob has been observed to stall rather than return an
/// error; without this bound a stalled join would wedge the supervisor task
/// forever (the hardware watchdog only sees a stalled executor, not a stalled
/// task).
pub const JOIN_TIMEOUT_MS: u64 = 15_000;

/// Maximum time to wait for one DHCP acquisition window.
pub const DHCP_TIMEOUT_MS: u64 = 15_000;

/// Number of DHCP windows to try within a single association before leaving.
///
/// Re-issuing DHCP on the existing association recovers faster than a full
/// disassociate/rejoin when the access point is up but slow to lease.
pub const DHCP_ATTEMPTS: u32 = 3;

/// Maximum time to wait for a `leave()` call before continuing regardless.
pub const LEAVE_TIMEOUT_MS: u64 = 5_000;

/// Initial reconnect backoff.
pub const INITIAL_BACKOFF_MS: u64 = 1_000;

/// Maximum reconnect backoff.
pub const MAX_BACKOFF_MS: u64 = 30_000;

/// Consecutive MQTT connect failures that request a Wi-Fi rejoin.
///
/// Covers the "associated but no traffic" case where the link never reports
/// down and so the supervisor alone would never rejoin.
pub const MQTT_REJOIN_FAILURES: u32 = 5;

/// Minimum duration an IP lease must remain up without link drop or rejoin
/// request before it refreshes [`NoIpWatchdog`] (prevents rapid link-flapping
/// and ~60 s MQTT rejoin cycles from resetting the 10-minute reboot timer).
pub const STABLE_LEASE_MS: u64 = 90_000;

/// Maximum time for DNS lookup, TCP connect, MQTT `CONNECT`/`CONNACK`
/// handshake, `SUBSCRIBE`, or `PUBLISH` flush.
pub const MQTT_IO_TIMEOUT_MS: u64 = 10_000;

/// TCP keep-alive probe interval on the MQTT `TcpSocket`.
pub const MQTT_TCP_KEEPALIVE_MS: u64 = 15_000;

/// Unacknowledged TCP retransmission timeout on the MQTT `TcpSocket`.
pub const MQTT_TCP_TIMEOUT_MS: u64 = 45_000;

/// Maximum time to wait for a `minimq` `Connection::poll()` progress event
/// (`60 s` keepalive + `15 s` grace), catching blackholed `PINGREQ` TCP flushes.
pub const MQTT_POLL_TIMEOUT_MS: u64 = 75_000;

/// Minimum elapsed session age at which a completed keepalive round-trip
/// (`idle_polls >= 3`) marks the MQTT session healthy.
pub const MQTT_HEALTHY_SESSION_MS: u64 = 55_000;

/// Maximum time either supervisor task (`wifi_supervisor_task` or
/// `telemetry_task`) may go without ticking its liveness heartbeat before
/// `watchdog::feeder_task` stops feeding the hardware watchdog.
pub const TASK_STALL_TIMEOUT_MS: u64 = 180_000;

/// Duration without an IP lease after which the MCU resets to recover a wedged
/// radio. Bounded: a long outage costs a reset every [`NO_IP_REBOOT_MS`], which
/// is harmless because the device cannot reach the network anyway.
pub const NO_IP_REBOOT_MS: u64 = 10 * 60 * 1000;

/// Tracks how long the device has been without an IP lease or healthy network
/// session.
#[derive(Debug, Clone, Copy)]
pub struct NoIpWatchdog {
    last_good_ms: Option<u64>,
}

impl NoIpWatchdog {
    /// Creates a watchdog that is already "due" once [`NO_IP_REBOOT_MS`] has
    /// elapsed since boot if no lease or healthy session is ever established.
    pub const fn new() -> Self {
        Self { last_good_ms: None }
    }

    /// Record `now_ms` as the last moment the link or session was known good.
    ///
    /// The countdown is measured from this instant, so it is refreshed when a
    /// lease has remained stable, when a held stable link is lost, and when an
    /// MQTT session proves end-to-end health. Refreshing on a stable link loss
    /// keeps a healthy association from being charged to the next outage:
    /// without it a link that stayed up longer than [`NO_IP_REBOOT_MS`] looks
    /// overdue the moment it drops, and the following re-association escalates
    /// to a reset.
    pub fn note_last_good(&mut self, now_ms: u64) {
        self.last_good_ms = Some(now_ms);
    }

    /// Record a link loss at `now_ms` for an association whose lease started at
    /// `lease_start_ms`.
    ///
    /// Only refreshes `last_good_ms` if the lease was held for at least
    /// [`STABLE_LEASE_MS`], preventing short-lived flapping leases from
    /// resetting the reboot countdown.
    pub fn note_link_loss(&mut self, lease_start_ms: u64, now_ms: u64) {
        if now_ms.saturating_sub(lease_start_ms) >= STABLE_LEASE_MS {
            self.note_last_good(now_ms);
        }
    }

    /// Returns `true` when [`NO_IP_REBOOT_MS`] has elapsed since the last
    /// known-good instant (a stable lease, a stable link loss, or a healthy
    /// session).
    pub fn reboot_due(&self, now_ms: u64) -> bool {
        let reference = self.last_good_ms.unwrap_or(0);
        now_ms.saturating_sub(reference) >= NO_IP_REBOOT_MS
    }
}

/// Returns `true` when an MQTT session has proven end-to-end health, either by
/// receiving an inbound application publish or by completing a full keepalive
/// cycle (`SUBACK` + `PINGREQ` + `PINGRESP`, i.e. `idle_polls >= 3`) after at
/// least [`MQTT_HEALTHY_SESSION_MS`] have elapsed since `connected_at_ms`.
pub const fn mqtt_session_healthy(
    connected_at_ms: u64,
    now_ms: u64,
    idle_polls: u32,
    inbound_publish: bool,
) -> bool {
    inbound_publish
        || (idle_polls >= 3 && now_ms.saturating_sub(connected_at_ms) >= MQTT_HEALTHY_SESSION_MS)
}

/// Returns `true` when a supervised task has not updated its heartbeat within
/// [`TASK_STALL_TIMEOUT_MS`].
pub const fn task_stalled(last_heartbeat_ms: u64, now_ms: u64) -> bool {
    now_ms.saturating_sub(last_heartbeat_ms) >= TASK_STALL_TIMEOUT_MS
}

impl Default for NoIpWatchdog {
    fn default() -> Self {
        Self::new()
    }
}

/// Counts consecutive MQTT connect failures and trips at a threshold.
#[derive(Debug, Clone, Copy)]
pub struct RejoinCounter {
    failures: u32,
}

impl RejoinCounter {
    /// Creates a counter with no recorded failures.
    pub const fn new() -> Self {
        Self { failures: 0 }
    }

    /// Record a failed connect attempt. Returns `true` exactly when the
    /// threshold is reached; the counter then resets so the next trip needs a
    /// fresh run of failures.
    pub fn failure(&mut self) -> bool {
        self.failures += 1;
        if self.failures >= MQTT_REJOIN_FAILURES {
            self.failures = 0;
            true
        } else {
            false
        }
    }

    /// Record a successful connection.
    pub fn success(&mut self) {
        self.failures = 0;
    }

    /// Current consecutive failure count.
    pub const fn failures(&self) -> u32 {
        self.failures
    }
}

impl Default for RejoinCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
