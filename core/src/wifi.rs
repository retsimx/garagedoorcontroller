//! Pure Wi-Fi recovery policy: timing thresholds and the small decision
//! helpers the firmware's network tasks need.
//!
//! Kept free of embassy/cyw43 types so the decision boundaries can be unit
//! tested on the host, in the same spirit as [`crate::door`].

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

/// Duration without an IP lease after which the MCU resets to recover a wedged
/// radio. Bounded: a long outage costs a reset every [`NO_IP_REBOOT_MS`], which
/// is harmless because the device cannot reach the network anyway.
pub const NO_IP_REBOOT_MS: u64 = 10 * 60 * 1000;

/// Tracks how long the device has been without an IP lease.
#[derive(Debug, Clone, Copy)]
pub struct NoIpWatchdog {
    last_ip_ms: Option<u64>,
}

impl NoIpWatchdog {
    /// Creates a watchdog that is already "due" once [`NO_IP_REBOOT_MS`] has
    /// elapsed since boot if no lease is ever acquired.
    pub const fn new() -> Self {
        Self { last_ip_ms: None }
    }

    /// Record that an IP lease is held at `now_ms`.
    pub fn note_ip(&mut self, now_ms: u64) {
        self.last_ip_ms = Some(now_ms);
    }

    /// Returns `true` when no lease has been held for at least
    /// [`NO_IP_REBOOT_MS`].
    pub fn reboot_due(&self, now_ms: u64) -> bool {
        let reference = self.last_ip_ms.unwrap_or(0);
        now_ms.saturating_sub(reference) >= NO_IP_REBOOT_MS
    }
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
