#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;

#[allow(dead_code)]
mod secrets;

/// Parse a bare ASCII integer at compile time.
const fn parse_u32(s: &str) -> u32 {
    let bytes = s.as_bytes();
    let mut n: u32 = 0;
    let mut i = 0;
    while i < bytes.len() {
        n = n * 10 + (bytes[i] - b'0') as u32;
        i += 1;
    }
    n
}

/// Integer version identity, parsed at compile time from the repo-root `VERSION`
/// file (injected by `app/build.rs` as `GARAGEDOOR_BUILD_VERSION`).
pub const VERSION: u32 = parse_u32(env!("GARAGEDOOR_BUILD_VERSION"));

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = embassy_rp::init(Default::default());

    defmt::info!("garagedoor-app starting, version={}", VERSION);
    defmt::info!("garagedoor-app peripherals initialized");

    let controller = garagedoor_core::DoorController::new(garagedoor_core::DoorState::Closed);
    defmt::info!(
        "initial door state: {:?}",
        defmt::Debug2Format(&controller.state())
    );
}
