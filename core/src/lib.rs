#![no_std]

//! Pure domain policy model for the garage door controller.
//!
//! This crate contains zero hardware or runtime dependencies and compiles cleanly
//! on host architectures (x86_64) as well as embedded bare-metal targets.

pub mod door;
pub mod flash;
pub mod mqtt;
pub mod ota;

pub use door::{
    DoorCommand, DoorState, ReedDebouncer, RelayAction, RelayInterlock, TriggerRejection,
    REED_DEBOUNCE_MS, RELAY_COOLDOWN_MS, RELAY_PULSE_MS,
};
pub use mqtt::{
    format_onchange, format_status_response, parse_status_request, EncodeError, StatusRequestError,
    TOPIC_ONCHANGE, TOPIC_RESET, TOPIC_STATUS, TOPIC_STATUS_RESPONSE, TOPIC_TRIGGER,
};
pub use ota::{
    apply_update, base64, basic_authorization, decide, parse_sha256_hex, parse_url, parse_version,
    BodyReader, Decision, Flasher, HeadError, HeadEvent, HeadParser, ResponseHead, UpdateError,
    Uri, UrlError, CHUNK_BYTES,
};

#[cfg(test)]
extern crate std;
