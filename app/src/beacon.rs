//! LED status beacon driven through CYW43 GPIO 0.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration};

use crate::radio::ControlMutex;

/// Status states signaled by the onboard LED beacon.
#[derive(Copy, Clone, Debug, PartialEq, Eq, defmt::Format)]
pub enum BeaconState {
    /// Radio is attempting to associate with the Wi-Fi AP or acquire DHCP lease.
    Connecting,
    /// Radio is associated and DHCP lease is active (normal operational heartbeat).
    Connected,
    /// Connection or protocol error code.
    Error(u8),
}

/// Global signal for dispatching beacon state updates asynchronously.
pub static BEACON_SIGNAL: Signal<CriticalSectionRawMutex, BeaconState> = Signal::new();

/// Brief, non-blocking helper to toggle CYW43 GPIO 0 and wait for duration or new signal.
async fn step_led(
    control: &'static ControlMutex,
    on: bool,
    duration: Duration,
    current_state: &mut BeaconState,
) {
    {
        let mut ctrl = control.lock().await;
        ctrl.gpio_set(0, on).await;
    }

    if let Ok(new_state) = with_timeout(duration, BEACON_SIGNAL.wait()).await {
        *current_state = new_state;
    }
}

/// Fast blink pattern: 100 ms ON / 100 ms OFF.
async fn step_connecting(control: &'static ControlMutex, current_state: &mut BeaconState) {
    step_led(control, true, Duration::from_millis(100), current_state).await;
    step_led(control, false, Duration::from_millis(100), current_state).await;
}

/// Heartbeat pulse pattern: 50 ms ON / 2000 ms OFF.
async fn step_connected(control: &'static ControlMutex, current_state: &mut BeaconState) {
    step_led(control, true, Duration::from_millis(50), current_state).await;
    step_led(control, false, Duration::from_millis(2000), current_state).await;
}

/// Diagnostic error sequence: `code` pulses (200 ms ON / 200 ms OFF) followed by 1000 ms pause.
async fn step_error(control: &'static ControlMutex, code: u8, current_state: &mut BeaconState) {
    let blinks = if code == 0 { 1 } else { code };
    for _ in 0..blinks {
        step_led(control, true, Duration::from_millis(200), current_state).await;
        step_led(control, false, Duration::from_millis(200), current_state).await;
        if !matches!(*current_state, BeaconState::Error(c) if c == code) {
            return;
        }
    }
    if matches!(*current_state, BeaconState::Error(c) if c == code) {
        step_led(control, false, Duration::from_millis(1000), current_state).await;
    }
}

/// Background beacon runner controlling the onboard LED on CYW43 GPIO 0.
#[embassy_executor::task]
pub async fn beacon_task(control: &'static ControlMutex) -> ! {
    let mut current_state = BeaconState::Connecting;

    loop {
        if let Some(new_state) = BEACON_SIGNAL.try_take() {
            current_state = new_state;
        }

        match current_state {
            BeaconState::Connecting => step_connecting(control, &mut current_state).await,
            BeaconState::Connected => step_connected(control, &mut current_state).await,
            BeaconState::Error(code) => step_error(control, code, &mut current_state).await,
        }
    }
}
