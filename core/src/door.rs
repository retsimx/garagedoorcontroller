//! Pure, time-injected door policy primitives.
//!
//! This module owns the garage door timing and state policy with zero hardware
//! or runtime dependencies, so it stays `no_std` and compiles on host targets as
//! well as bare-metal ones. Every time-dependent decision takes an explicit
//! millisecond timestamp (`now_ms`); callers supply time from their runtime while
//! host tests drive a mock clock to exercise exact boundary behaviour.

/// Duration in milliseconds that the relay stays asserted for a single trigger.
pub const RELAY_PULSE_MS: u32 = 600;
/// Minimum cooldown in milliseconds enforced after a pulse before the next trigger.
pub const RELAY_COOLDOWN_MS: u32 = 2000;
/// Milliseconds a reed level must be stable before the new state commits.
pub const REED_DEBOUNCE_MS: u32 = 50;

/// Confirmed state of the garage door as reported by the reed switch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorState {
    /// Reed contact open / magnet away (GPIO 21 HIGH).
    Open,
    /// Reed contact closed / magnet near (GPIO 21 LOW).
    Closed,
}

/// Commands accepted by the door actuator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorCommand {
    /// Request a relay pulse.
    Trigger,
    /// Request the current door state (a no-op for the actuator).
    QueryStatus,
}

/// Hardware action requested by the relay policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayAction {
    /// Assert the relay output high to begin a pulse.
    AssertHigh,
    /// Deassert the relay output low to end a pulse.
    DeassertLow,
}

/// Reason a `DoorCommand::Trigger` was rejected by the relay interlock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerRejection {
    /// A pulse is currently active.
    PulseActive,
    /// The mandatory cooldown after the previous pulse has not elapsed.
    Cooldown {
        /// Milliseconds remaining until a trigger would be accepted.
        remaining_ms: u32,
    },
}

/// Enforces relay pulse bounds: one active pulse at a time plus a mandatory cooldown.
#[derive(Debug, Clone)]
pub struct RelayInterlock {
    pulse_end_ms: Option<u64>,
    cooldown_end_ms: Option<u64>,
}

impl RelayInterlock {
    /// Creates an idle interlock with no pulse armed and no cooldown active.
    pub const fn new() -> Self {
        Self {
            pulse_end_ms: None,
            cooldown_end_ms: None,
        }
    }

    /// Attempts to start a relay pulse at `now_ms`.
    ///
    /// Rejects while a pulse is active, then while the post-pulse cooldown is
    /// still running; otherwise arms a fresh pulse, clears the cooldown, and
    /// returns the [`RelayAction::AssertHigh`] the caller must apply to the
    /// relay output.
    pub fn trigger(&mut self, now_ms: u64) -> Result<RelayAction, TriggerRejection> {
        if self.is_pulsing(now_ms) {
            return Err(TriggerRejection::PulseActive);
        }
        if let Some(cooldown_end_ms) = self.cooldown_end_ms.filter(|end_ms| now_ms < *end_ms) {
            return Err(TriggerRejection::Cooldown {
                remaining_ms: (cooldown_end_ms - now_ms) as u32,
            });
        }
        self.pulse_end_ms = Some(now_ms + RELAY_PULSE_MS as u64);
        self.cooldown_end_ms = None;
        Ok(RelayAction::AssertHigh)
    }

    /// Reconciles the interlock at `now_ms`, ending a pulse that has elapsed.
    ///
    /// Returns `Some(RelayAction::DeassertLow)` exactly once when a pulse ends.
    /// The cooldown deadline derives from the scheduled pulse end, not from the
    /// observation time, so the timing stays deterministic.
    pub fn poll(&mut self, now_ms: u64) -> Option<RelayAction> {
        match self.pulse_end_ms {
            Some(end_ms) if now_ms >= end_ms => {
                self.pulse_end_ms = None;
                self.cooldown_end_ms = Some(end_ms + RELAY_COOLDOWN_MS as u64);
                Some(RelayAction::DeassertLow)
            }
            _ => None,
        }
    }

    /// Returns `true` if a pulse is armed and has not yet elapsed at `now_ms`.
    pub const fn is_pulsing(&self, now_ms: u64) -> bool {
        match self.pulse_end_ms {
            Some(end_ms) => now_ms < end_ms,
            None => false,
        }
    }
}

impl Default for RelayInterlock {
    fn default() -> Self {
        Self::new()
    }
}

/// Debounces raw reed switch samples into committed [`DoorState`] transitions.
#[derive(Debug, Clone)]
pub struct ReedDebouncer {
    committed: DoorState,
    pending: Option<(DoorState, u64)>,
}

impl ReedDebouncer {
    /// Creates a debouncer whose committed state is `initial`, with no candidate pending.
    pub const fn new(initial: DoorState) -> Self {
        Self {
            committed: initial,
            pending: None,
        }
    }

    /// Returns the last committed door state.
    pub const fn state(&self) -> DoorState {
        self.committed
    }

    /// Returns `true` if a candidate state is awaiting the debounce interval.
    pub const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Feeds a raw reed sample at `now_ms`.
    ///
    /// `reed_closed == true` maps to [`DoorState::Closed`]. Returns `Some(state)`
    /// only when a change has been stable for [`REED_DEBOUNCE_MS`] and is thus
    /// committed; bouncing samples reset the pending candidate without emitting.
    pub fn update(&mut self, reed_closed: bool, now_ms: u64) -> Option<DoorState> {
        let desired = if reed_closed {
            DoorState::Closed
        } else {
            DoorState::Open
        };

        if desired == self.committed {
            self.pending = None;
            return None;
        }

        let stable = matches!(
            self.pending,
            Some((state, since_ms))
                if state == desired && now_ms.saturating_sub(since_ms) >= REED_DEBOUNCE_MS as u64
        );
        if stable {
            self.committed = desired;
            self.pending = None;
            return Some(desired);
        }

        self.pending = Some((desired, now_ms));
        None
    }
}

#[cfg(test)]
mod tests;
