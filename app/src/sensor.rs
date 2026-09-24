//! Reed switch monitor task: debounces GPIO 21 and broadcasts committed state.

use embassy_rp::gpio::Input;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Timer};
use garagedoor_core::{DoorState, ReedDebouncer, REED_DEBOUNCE_MS};

/// Latest committed door state, signalled on change for consumers (e.g. MQTT).
pub static DOOR_STATE_SIGNAL: Signal<CriticalSectionRawMutex, DoorState> = Signal::new();

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Owns GPIO 21: samples the reed switch on edges, debounces changes, and
/// signals the committed [`DoorState`] only when it actually changes.
#[embassy_executor::task]
pub async fn sensor_task(mut reed: Input<'static>) -> ! {
    // Legacy MicroPython parity: GPIO 21 HIGH means the door is CLOSED and LOW
    // means OPEN (`False if reed_pin.value() else True` in the old firmware).
    let initial = if reed.is_high() {
        DoorState::Closed
    } else {
        DoorState::Open
    };
    let mut debouncer = ReedDebouncer::new(initial);
    DOOR_STATE_SIGNAL.signal(initial);
    defmt::info!("door state: {:?}", defmt::Debug2Format(&initial));

    loop {
        let closed = reed.is_high();
        if let Some(state) = debouncer.update(closed, now_ms()) {
            DOOR_STATE_SIGNAL.signal(state);
            defmt::info!("door state: {:?}", defmt::Debug2Format(&state));
        }

        if debouncer.is_pending() {
            Timer::after(Duration::from_millis(REED_DEBOUNCE_MS as u64)).await;
        } else {
            // Level wait derived from the last sample: a level wait resolves
            // immediately if the pin has already moved, so an edge landing between
            // the sample above and the wait can never be lost. The timeout still
            // re-samples at least every debounce interval as a safety net.
            let debounce = Duration::from_millis(REED_DEBOUNCE_MS as u64);
            if closed {
                let _ = with_timeout(debounce, reed.wait_for_low()).await;
            } else {
                let _ = with_timeout(debounce, reed.wait_for_high()).await;
            }
        }
    }
}
