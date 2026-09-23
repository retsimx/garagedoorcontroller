//! Pure post-swap self-test policy.
//!
//! After a bootloader swap, the running image is unconfirmed. This module
//! decides whether to permanently confirm it. The policy is **fail-closed on
//! hardware**: any mandatory hardware check that fails withholds confirmation so
//! the hardware watchdog expires and the bootloader reverts. The network check
//! is **best-effort**: a Wi-Fi or DHCP outage is informational and never reverts
//! a hardware-healthy image (grace policy).

/// Maximum post-swap self-test window, in milliseconds.
pub const SELF_TEST_WINDOW_MS: u64 = 30_000;

/// Signals observed during the post-swap self-test window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelfTestSignals {
    /// GPIO 18 (relay actuator) configured and held LOW (read-only).
    pub relay_ok: bool,
    /// GPIO 21 (reed switch) sampled as a valid boolean.
    pub reed_ok: bool,
    /// Wi-Fi associated and a DHCP lease acquired (best-effort).
    pub network_ok: bool,
}

/// Whether to permanently confirm the running image or let it revert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfTestDecision {
    /// All mandatory checks passed; call `mark_booted()`.
    Commit,
    /// A mandatory check failed; withhold confirmation so the watchdog reverts.
    Revert,
}

/// Evaluate the self-test signals.
///
/// Mandatory hardware checks are fail-closed: any hardware failure reverts.
/// The network check is best-effort: a Wi-Fi outage never reverts a good image.
pub fn evaluate(signals: &SelfTestSignals) -> SelfTestDecision {
    if signals.relay_ok && signals.reed_ok {
        SelfTestDecision::Commit
    } else {
        SelfTestDecision::Revert
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(relay_ok: bool, reed_ok: bool, network_ok: bool) -> SelfTestSignals {
        SelfTestSignals {
            relay_ok,
            reed_ok,
            network_ok,
        }
    }

    #[test]
    fn all_true_commits() {
        assert_eq!(
            evaluate(&signals(true, true, true)),
            SelfTestDecision::Commit
        );
    }

    #[test]
    fn relay_failure_reverts() {
        assert_eq!(
            evaluate(&signals(false, true, true)),
            SelfTestDecision::Revert
        );
    }

    #[test]
    fn reed_failure_reverts() {
        assert_eq!(
            evaluate(&signals(true, false, true)),
            SelfTestDecision::Revert
        );
    }

    #[test]
    fn both_hardware_failures_revert() {
        assert_eq!(
            evaluate(&signals(false, false, true)),
            SelfTestDecision::Revert
        );
    }

    #[test]
    fn network_failure_only_commits() {
        assert_eq!(
            evaluate(&signals(true, true, false)),
            SelfTestDecision::Commit
        );
    }

    #[test]
    fn window_is_thirty_seconds() {
        assert_eq!(SELF_TEST_WINDOW_MS, 30_000);
    }
}
