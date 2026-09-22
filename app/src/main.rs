#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;

mod beacon;
mod radio;
#[allow(dead_code)]
mod secrets;
mod wifi;

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
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    defmt::info!("garagedoor-app starting, version={}", VERSION);
    defmt::info!("garagedoor-app peripherals initialized");

    let controller = garagedoor_core::DoorController::new(garagedoor_core::DoorState::Closed);
    defmt::info!(
        "initial door state: {:?}",
        defmt::Debug2Format(&controller.state())
    );

    let radio_peripherals = radio::RadioPeripherals {
        pwr: p.PIN_23,
        cs: p.PIN_25,
        dio: p.PIN_24,
        clk: p.PIN_29,
        pio: p.PIO0,
        dma_ch0: p.DMA_CH0,
        dma_ch1: p.DMA_CH1,
    };

    let (control, stack) = radio::init(spawner, radio_peripherals).await;
    defmt::info!("radio and network stack initialized");

    spawner.spawn(defmt::unwrap!(beacon::beacon_task(control)));
    spawner.spawn(defmt::unwrap!(wifi::wifi_supervisor_task(control, stack)));
}
