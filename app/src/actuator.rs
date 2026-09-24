//! Door actuator task: bounds a single GPIO 18 relay pulse and enforces the
//! interlock/cooldown policy owned by `garagedoor_core`.

use embassy_rp::gpio::Output;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::{Channel, Receiver};
use embassy_time::{Duration, Instant, Timer};
use garagedoor_core::{DoorCommand, RelayAction, RelayInterlock, RELAY_PULSE_MS};

/// Commands for the door actuator. Capacity 4 lets trigger bursts queue and be
/// drained into a single interlocked pulse.
pub static DOOR_CMD_CHANNEL: Channel<CriticalSectionRawMutex, DoorCommand, 4> = Channel::new();

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Applies a policy-issued [`RelayAction`] to the relay output.
fn apply(relay: &mut Output<'static>, action: RelayAction) {
    match action {
        RelayAction::AssertHigh => relay.set_high(),
        RelayAction::DeassertLow => relay.set_low(),
    }
}

/// Owns GPIO 18: asserts the relay for exactly [`RELAY_PULSE_MS`] per accepted
/// trigger, and rejects triggers while a pulse or cooldown is active so the
/// relay can never be held on.
#[embassy_executor::task]
pub async fn actuator_task(
    mut relay: Output<'static>,
    rx: Receiver<'static, CriticalSectionRawMutex, DoorCommand, 4>,
) -> ! {
    // Start deasserted so boot can never fire the door.
    relay.set_low();
    let mut interlock = RelayInterlock::new();

    loop {
        match rx.receive().await {
            DoorCommand::Trigger => {
                let now = now_ms();
                if let Some(action) = interlock.poll(now) {
                    apply(&mut relay, action);
                }

                match interlock.trigger(now) {
                    Ok(action) => {
                        // Timestamp the assertion so the bench can measure the true
                        // high window (the pad is driven directly by `apply`).
                        let asserted_at = Instant::now();
                        apply(&mut relay, action);
                        Timer::after(Duration::from_millis(RELAY_PULSE_MS as u64)).await;
                        // De-assert unconditionally once the pulse window has elapsed
                        // so a sub-tick boundary can never leave the relay asserted,
                        // then reconcile the interlock to arm the cooldown.
                        apply(&mut relay, RelayAction::DeassertLow);
                        let pulse_us = (Instant::now() - asserted_at).as_micros();
                        let _ = interlock.poll(now_ms());
                        defmt::info!("relay pulse complete pulse_us={}", pulse_us);
                    }
                    Err(reason) => {
                        // Interlock holds: leave the pin untouched.
                        defmt::warn!("trigger rejected: {:?}", defmt::Debug2Format(&reason));
                    }
                }
            }
            DoorCommand::QueryStatus => {
                // State is broadcast by the sensor task via `DOOR_STATE_SIGNAL`.
                defmt::debug!("query status ignored; door state is sensor-broadcast");
            }
        }
    }
}
