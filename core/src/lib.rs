#![no_std]

//! Pure domain policy model for the garage door controller.
//!
//! This crate contains zero hardware or runtime dependencies and compiles cleanly
//! on host architectures (x86_64) as well as embedded bare-metal targets.

pub mod door;
pub mod flash;

pub use door::{
    DoorCommand, DoorState, ReedDebouncer, RelayAction, RelayInterlock, TriggerRejection,
    REED_DEBOUNCE_MS, RELAY_COOLDOWN_MS, RELAY_PULSE_MS,
};

#[cfg(test)]
extern crate std;
