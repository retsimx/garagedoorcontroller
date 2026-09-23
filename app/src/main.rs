#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::watchdog::{ResetReason, Watchdog};
use embassy_time::{Duration, Timer};

mod actuator;
mod beacon;
mod radio;
#[allow(dead_code)]
mod secrets;
mod sensor;
pub mod update;
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

    let mut wd = Watchdog::new(p.WATCHDOG);
    let reset_reason = match wd.reset_reason() {
        Some(ResetReason::Forced) => "watchdog-forced",
        Some(ResetReason::TimedOut) => "watchdog-timeout",
        None => "power-on-or-debugger",
    };

    defmt::info!("garagedoor-app starting, version={}", VERSION);
    defmt::info!("reset_reason={}", reset_reason);
    defmt::info!("garagedoor-app peripherals initialized");

    wd.start(update::WATCHDOG_TIMEOUT);
    update::init_watchdog(wd);
    spawner.spawn(defmt::unwrap!(watchdog_task()));

    let relay = Output::new(p.PIN_18, Level::Low);
    let reed = Input::new(p.PIN_21, Pull::Up);
    spawner.spawn(defmt::unwrap!(actuator::actuator_task(
        relay,
        actuator::DOOR_CMD_CHANNEL.receiver()
    )));
    spawner.spawn(defmt::unwrap!(sensor::sensor_task(reed)));

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

#[embassy_executor::task]
async fn watchdog_task() -> ! {
    loop {
        update::feed(update::WATCHDOG_TIMEOUT);
        Timer::after(Duration::from_millis(500)).await;
    }
}
