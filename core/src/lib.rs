#![no_std]

//! Pure domain policy model for the garage door controller.
//!
//! This crate contains zero hardware or runtime dependencies and compiles cleanly
//! on host architectures (x86_64) as well as embedded bare-metal targets.

pub mod flash;

/// Current state of the garage door.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorState {
    /// Door is completely closed (confirmed by reed switch contact).
    Closed,
    /// Door is currently in motion moving towards the open position.
    Opening,
    /// Door is fully open (or reed switch contact broken and completed travel).
    Open,
    /// Door is currently in motion moving towards the closed position.
    Closing,
    /// Door movement was interrupted midway.
    Stopped,
}

/// Commands accepted by the door controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoorCommand {
    /// Command to open the door.
    Open,
    /// Command to close the door.
    Close,
    /// Command to toggle door motion (single pulse behavior).
    Toggle,
    /// Command to stop the door midway during movement.
    Stop,
}

/// Hardware action requested by the domain policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayAction {
    /// Pulse the door opener relay for the specified duration in milliseconds.
    PulseTrigger {
        /// Pulse duration in milliseconds (standard: 600ms).
        duration_ms: u32,
    },
}

/// Standard duration in milliseconds for pulsing the garage door relay.
pub const DEFAULT_RELAY_PULSE_MS: u32 = 600;

/// Domain events emitted on state transitions or command processing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DoorEvent {
    /// The door state transitioned from one state to another.
    StateTransition {
        /// Previous state before transition.
        from: DoorState,
        /// New state after transition.
        to: DoorState,
    },
    /// An action on the relay was triggered.
    ActionTriggered(RelayAction),
    /// A command was rejected or ignored because the door is already in the target state.
    CommandIgnored {
        /// State at the time of rejection.
        state: DoorState,
        /// Command that was ignored.
        command: DoorCommand,
    },
}

/// Garage door domain controller tracking state and applying transition policies.
#[derive(Debug, Clone)]
pub struct DoorController {
    state: DoorState,
}

impl DoorController {
    /// Create a new door controller initialized to the specified state.
    pub const fn new(initial_state: DoorState) -> Self {
        Self {
            state: initial_state,
        }
    }

    /// Returns the current state of the door.
    pub const fn state(&self) -> DoorState {
        self.state
    }

    /// Whether the door is confirmed closed.
    pub const fn is_closed(&self) -> bool {
        matches!(self.state, DoorState::Closed)
    }

    /// Whether the door is open (not closed).
    pub const fn is_open(&self) -> bool {
        !self.is_closed()
    }

    /// Handle a command according to garage door operational policies.
    ///
    /// Returns `Some(RelayAction)` if the opener relay needs to be pulsed,
    /// or `None` if the command is a no-op / ignored for the current state.
    pub fn handle_command(&mut self, cmd: DoorCommand) -> Option<RelayAction> {
        match cmd {
            DoorCommand::Open => self.handle_open(),
            DoorCommand::Close => self.handle_close(),
            DoorCommand::Toggle => self.handle_toggle(),
            DoorCommand::Stop => self.handle_stop(),
        }
    }

    fn handle_open(&mut self) -> Option<RelayAction> {
        match self.state {
            DoorState::Closed | DoorState::Stopped => {
                self.state = DoorState::Opening;
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                })
            }
            DoorState::Closing => {
                // Reversing direction while closing
                self.state = DoorState::Opening;
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                })
            }
            DoorState::Open | DoorState::Opening => None,
        }
    }

    fn handle_close(&mut self) -> Option<RelayAction> {
        match self.state {
            DoorState::Open | DoorState::Stopped => {
                self.state = DoorState::Closing;
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                })
            }
            DoorState::Opening => {
                // Stopping or reversing while opening
                self.state = DoorState::Closing;
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                })
            }
            DoorState::Closed | DoorState::Closing => None,
        }
    }

    fn handle_toggle(&mut self) -> Option<RelayAction> {
        let (next_state, action) = match self.state {
            DoorState::Closed => (
                DoorState::Opening,
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                }),
            ),
            DoorState::Open => (
                DoorState::Closing,
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                }),
            ),
            DoorState::Opening | DoorState::Closing => (
                DoorState::Stopped,
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                }),
            ),
            DoorState::Stopped => (
                DoorState::Closing,
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                }),
            ),
        };
        self.state = next_state;
        action
    }

    fn handle_stop(&mut self) -> Option<RelayAction> {
        match self.state {
            DoorState::Opening | DoorState::Closing => {
                self.state = DoorState::Stopped;
                Some(RelayAction::PulseTrigger {
                    duration_ms: DEFAULT_RELAY_PULSE_MS,
                })
            }
            DoorState::Closed | DoorState::Open | DoorState::Stopped => None,
        }
    }

    /// Update controller state based on reed switch contact sensor input.
    ///
    /// `reed_closed == true` indicates the door magnet is in contact with the reed switch (closed).
    /// `reed_closed == false` indicates the door has moved away from the closed position.
    pub fn handle_sensor_update(&mut self, reed_closed: bool) -> Option<DoorEvent> {
        let old_state = self.state;
        if reed_closed {
            if self.state != DoorState::Closed {
                self.state = DoorState::Closed;
                return Some(DoorEvent::StateTransition {
                    from: old_state,
                    to: DoorState::Closed,
                });
            }
        } else if self.state == DoorState::Closed {
            // Door broke contact with the sensor
            self.state = DoorState::Opening;
            return Some(DoorEvent::StateTransition {
                from: old_state,
                to: DoorState::Opening,
            });
        }
        None
    }

    /// Manually transition the door to `Open` when motion travel finishes.
    pub fn mark_fully_open(&mut self) -> Option<DoorEvent> {
        let old_state = self.state;
        if self.state == DoorState::Opening || self.state == DoorState::Stopped {
            self.state = DoorState::Open;
            Some(DoorEvent::StateTransition {
                from: old_state,
                to: DoorState::Open,
            })
        } else {
            None
        }
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state() {
        let controller = DoorController::new(DoorState::Closed);
        assert_eq!(controller.state(), DoorState::Closed);
        assert!(controller.is_closed());
        assert!(!controller.is_open());
    }

    #[test]
    fn test_open_command_from_closed() {
        let mut controller = DoorController::new(DoorState::Closed);
        let action = controller.handle_command(DoorCommand::Open);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Opening);
        assert!(!controller.is_closed());
        assert!(controller.is_open());
    }

    #[test]
    fn test_open_command_when_already_open() {
        let mut controller = DoorController::new(DoorState::Open);
        let action = controller.handle_command(DoorCommand::Open);
        assert_eq!(action, None);
        assert_eq!(controller.state(), DoorState::Open);
    }

    #[test]
    fn test_close_command_from_open() {
        let mut controller = DoorController::new(DoorState::Open);
        let action = controller.handle_command(DoorCommand::Close);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Closing);
    }

    #[test]
    fn test_close_command_when_already_closed() {
        let mut controller = DoorController::new(DoorState::Closed);
        let action = controller.handle_command(DoorCommand::Close);
        assert_eq!(action, None);
        assert_eq!(controller.state(), DoorState::Closed);
    }

    #[test]
    fn test_stop_command_during_movement() {
        let mut controller = DoorController::new(DoorState::Opening);
        let action = controller.handle_command(DoorCommand::Stop);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Stopped);

        let mut controller_closing = DoorController::new(DoorState::Closing);
        let action_closing = controller_closing.handle_command(DoorCommand::Stop);
        assert_eq!(
            action_closing,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller_closing.state(), DoorState::Stopped);
    }

    #[test]
    fn test_stop_command_when_idle() {
        let mut controller = DoorController::new(DoorState::Closed);
        assert_eq!(controller.handle_command(DoorCommand::Stop), None);

        let mut controller_open = DoorController::new(DoorState::Open);
        assert_eq!(controller_open.handle_command(DoorCommand::Stop), None);
    }

    #[test]
    fn test_toggle_command_transitions() {
        let mut controller = DoorController::new(DoorState::Closed);
        // Closed -> Toggle -> Opening
        let action = controller.handle_command(DoorCommand::Toggle);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Opening);

        // Opening -> Toggle -> Stopped
        let action = controller.handle_command(DoorCommand::Toggle);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Stopped);

        // Stopped -> Toggle -> Closing
        let action = controller.handle_command(DoorCommand::Toggle);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Closing);

        // Closing -> Toggle -> Stopped
        let action = controller.handle_command(DoorCommand::Toggle);
        assert_eq!(
            action,
            Some(RelayAction::PulseTrigger {
                duration_ms: DEFAULT_RELAY_PULSE_MS
            })
        );
        assert_eq!(controller.state(), DoorState::Stopped);
    }

    #[test]
    fn test_sensor_updates() {
        let mut controller = DoorController::new(DoorState::Opening);

        // Contact made -> Closed
        let event = controller.handle_sensor_update(true);
        assert_eq!(
            event,
            Some(DoorEvent::StateTransition {
                from: DoorState::Opening,
                to: DoorState::Closed,
            })
        );
        assert_eq!(controller.state(), DoorState::Closed);

        // Repeated contact made -> None
        let event = controller.handle_sensor_update(true);
        assert_eq!(event, None);

        // Contact broken from closed -> Opening
        let event = controller.handle_sensor_update(false);
        assert_eq!(
            event,
            Some(DoorEvent::StateTransition {
                from: DoorState::Closed,
                to: DoorState::Opening,
            })
        );
        assert_eq!(controller.state(), DoorState::Opening);

        // Mark fully open
        let event = controller.mark_fully_open();
        assert_eq!(
            event,
            Some(DoorEvent::StateTransition {
                from: DoorState::Opening,
                to: DoorState::Open,
            })
        );
        assert_eq!(controller.state(), DoorState::Open);
    }

    #[test]
    fn test_mark_fully_open_when_closing() {
        let mut controller = DoorController::new(DoorState::Closing);
        let event = controller.mark_fully_open();
        assert_eq!(event, None);
        assert_eq!(controller.state(), DoorState::Closing);
    }
}
