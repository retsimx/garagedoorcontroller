//! Wi-Fi station association and reconnect supervision loop.

use embassy_time::{with_timeout, Duration, Timer};

use crate::beacon::{self, BeaconState};
use crate::radio::{ControlMutex, NetStack};
use crate::secrets;

/// Maximum reconnect backoff delay in seconds.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Initial reconnect backoff delay in seconds.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Maximum duration to wait for DHCP lease acquisition after Wi-Fi association.
const DHCP_TIMEOUT: Duration = Duration::from_secs(15);

/// Attempt to join the configured Wi-Fi network.
async fn attempt_join(control: &'static ControlMutex) -> Result<(), cyw43::JoinError> {
    let mut ctrl = control.lock().await;
    let options = if secrets::WIFI_PASSWORD.is_empty() {
        cyw43::JoinOptions::new_open()
    } else {
        cyw43::JoinOptions::new(secrets::WIFI_PASSWORD.as_bytes())
    };
    ctrl.join(secrets::WIFI_SSID, options).await
}

/// Log acquired IPv4 configuration.
fn log_ip_config(stack: NetStack) {
    if let Some(cfg) = stack.config_v4() {
        defmt::info!("wifi: IP lease acquired!");
        defmt::info!("wifi: IP address: {}", defmt::Debug2Format(&cfg.address));
        if let Some(gateway) = cfg.gateway {
            defmt::info!("wifi: Gateway: {}", defmt::Debug2Format(&gateway));
        }
    }
}

/// Await DHCP configuration and supervise the active network link.
async fn handle_associated(stack: NetStack, backoff: &mut Duration) {
    defmt::info!(
        "wifi: association successful to SSID: {}, awaiting DHCP lease...",
        secrets::WIFI_SSID
    );

    match with_timeout(DHCP_TIMEOUT, stack.wait_config_up()).await {
        Ok(()) => {
            log_ip_config(stack);
            beacon::BEACON_SIGNAL.signal(BeaconState::Connected);

            // Reset exponential backoff on successful connection
            *backoff = INITIAL_BACKOFF;

            // Monitor connection link; blocks until link is lost
            stack.wait_link_down().await;
            defmt::warn!("wifi: network link dropped!");
            beacon::BEACON_SIGNAL.signal(BeaconState::Error(1));
        }
        Err(_) => {
            defmt::warn!("wifi: DHCP configuration timed out");
            beacon::BEACON_SIGNAL.signal(BeaconState::Error(1));
        }
    }
}

/// Leave the current association and wait with exponential backoff before the next attempt.
async fn leave_and_backoff(control: &'static ControlMutex, backoff: &mut Duration) {
    {
        let mut ctrl = control.lock().await;
        ctrl.leave().await;
    }

    defmt::info!(
        "wifi: backing off for {}s before next retry...",
        backoff.as_secs()
    );
    Timer::after(*backoff).await;
    *backoff = core::cmp::min(*backoff * 2, MAX_BACKOFF);
}

/// Background Wi-Fi supervisor task.
///
/// Invariant: Wi-Fi connection loss, association failure, or DHCP timeout
/// will NEVER panic, reset, or restart the RP2040 MCU. Offline door
/// control operations remain completely unaffected.
#[embassy_executor::task]
pub async fn wifi_supervisor_task(control: &'static ControlMutex, stack: NetStack) -> ! {
    let mut backoff = INITIAL_BACKOFF;

    loop {
        defmt::info!(
            "wifi: initiating association to SSID: {}",
            secrets::WIFI_SSID
        );
        beacon::BEACON_SIGNAL.signal(BeaconState::Connecting);

        match attempt_join(control).await {
            Ok(()) => handle_associated(stack, &mut backoff).await,
            Err(err) => {
                defmt::warn!("wifi: association failed: {:?}", defmt::Debug2Format(&err));
                beacon::BEACON_SIGNAL.signal(BeaconState::Error(1));
            }
        }

        leave_and_backoff(control, &mut backoff).await;
    }
}
