#![no_std]
#![no_main]

use defmt_rtt as _;
use panic_probe as _;

use embassy_boot::State;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_time::{with_timeout, Duration};
use garagedoor_core::ota::selftest::{
    evaluate, SelfTestDecision, SelfTestSignals, SELF_TEST_WINDOW_MS,
};

mod actuator;
mod beacon;
mod heap;
mod ota;
mod radio;
#[allow(dead_code)]
mod secrets;
mod sensor;
mod telemetry;
pub mod update;
mod watchdog;
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
    // The static heap must be ready before anything can allocate: embedded-tls's
    // RSA handshake verification allocates.
    heap::init();

    let p = embassy_rp::init(Default::default());

    // Arm the watchdog before any driver initialisation, then feed it forever.
    watchdog::init(p.WATCHDOG);
    spawner.spawn(defmt::unwrap!(watchdog::feeder_task()));

    defmt::info!("garagedoor-app starting, version={}", VERSION);

    // Deassert the relay as the first actuator action on every boot.
    let relay = Output::new(p.PIN_18, Level::Low);

    // Deliberately-faulty image for automated rollback verification.
    #[cfg(feature = "selftest-broken")]
    panic!("simulated boot failure");

    // No internal pull: the legacy firmware used a bare input (`Pin(21, IN)`),
    // relying on the board's external reed network to define the level.
    let reed = Input::new(p.PIN_21, Pull::None);

    // Boot disposition, read once from the embassy-boot state.
    let mut updater = update::Updater::new(p.FLASH);
    let boot_state = updater.get_state().await;
    let is_swap = matches!(boot_state, Ok(State::Swap));
    let is_revert = matches!(boot_state, Ok(State::Revert));
    if is_revert {
        // Deliberate: do NOT clear the REVERT marker here. `mark_booted()`
        // would normalise the state to `Boot`, so the next reset would see
        // `is_revert == false` and re-download the same failed image — the
        // retry loop returns. Leaving the marker set keeps the boot check
        // suppressed until a manual `garagedoor/reset` performs a successful
        // update (which sets SWAP magic and eventually BOOT on confirmation).
        defmt::warn!("ota_previous_image_restored");
    }

    // Mandatory hardware checks — read-only on GPIO 18 — only on a swap.
    let mut signals = SelfTestSignals {
        relay_ok: true,
        reed_ok: true,
        network_ok: false,
    };
    if is_swap {
        signals.relay_ok = relay.is_set_low();
        // `is_low()` and `is_high()` are infallible and mutually exclusive, so
        // this is always true; it proves the pin is configured and readable,
        // not that the switch is healthy.
        signals.reed_ok = reed.is_low() || reed.is_high();
        if evaluate(&signals) == SelfTestDecision::Revert {
            defmt::error!(
                "selftest_hw_fail relay={} reed={}",
                signals.relay_ok,
                signals.reed_ok
            );
            revert_hang();
        }
    }

    // Door control is live before any network wait.
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
    spawner.spawn(defmt::unwrap!(telemetry::telemetry_task(stack)));

    // Best-effort network check, then confirm or revert.
    if is_swap {
        signals.network_ok = with_timeout(
            Duration::from_millis(SELF_TEST_WINDOW_MS),
            stack.wait_config_up(),
        )
        .await
        .is_ok();
        match evaluate(&signals) {
            SelfTestDecision::Commit => {
                defmt::info!("selftest_passed network={}", signals.network_ok);
                match updater.mark_booted().await {
                    Ok(()) => defmt::info!("ota_booted"),
                    // Confirmation failed, so the image stays unconfirmed and the
                    // bootloader will revert it on the next reset. Keep running so
                    // door control stays available until then.
                    Err(_) => defmt::error!("ota_mark_booted_failed"),
                }
            }
            SelfTestDecision::Revert => revert_hang(),
        }
    }

    ota::spawn(spawner, stack, updater, !is_revert);
}

/// Stop feeding the watchdog and spin so the 8 s hardware watchdog resets the
/// MCU; the bootloader then reverts the unconfirmed image.
fn revert_hang() -> ! {
    defmt::error!("selftest_revert_hang");
    loop {
        core::hint::spin_loop();
    }
}
