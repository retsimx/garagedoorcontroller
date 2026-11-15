//! Wi-Fi station association and reconnect supervision loop.
//!
//! Recovery is bounded at every step so no single stalled radio call can wedge
//! the task:
//!
//! - the join and leave calls are timeout-wrapped (the CYW43 blob has been
//!   observed to stall rather than return an error),
//! - DHCP is retried in place a bounded number of times before leaving, so a
//!   slow-but-working lease does not cost a full rejoin,
//! - the link is watched for loss *and* for a rejoin request raised by the
//!   telemetry task after repeated MQTT failures (the "associated but dead"
//!   case the link state alone never reports),
//! - if no IP lease is held for a long period the MCU resets to recover a
//!   wedged radio; this is the one deliberate exception to "network loss never
//!   resets", bounded and harmless because the device cannot reach the network
//!   anyway.

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{with_timeout, Duration, Instant, Timer};
use garagedoor_core::wifi::{
    NoIpWatchdog, DHCP_ATTEMPTS, DHCP_TIMEOUT_MS, INITIAL_BACKOFF_MS, JOIN_TIMEOUT_MS,
    LEAVE_TIMEOUT_MS, MAX_BACKOFF_MS, NO_IP_REBOOT_MS, STABLE_LEASE_MS,
};

use crate::beacon::{self, BeaconState};
use crate::radio::{ControlMutex, NetStack};
use crate::secrets;

/// Raised by the telemetry task when repeated MQTT connect failures suggest the
/// link is associated but no longer passing traffic.
pub static WIFI_REJOIN_SIGNAL: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Why one join attempt did not succeed.
enum JoinAttemptError {
    /// The join call did not complete within [`JOIN_TIMEOUT_MS`].
    Timeout,
    /// The radio reported a join failure.
    Join(cyw43::JoinError),
}

/// One join attempt with an explicit authentication mode, bounded by a timeout.
///
/// `auth` is ignored for open networks, which the driver joins with
/// [`cyw43::JoinAuth::Open`].
async fn join_with(
    control: &'static ControlMutex,
    auth: cyw43::JoinAuth,
) -> Result<(), JoinAttemptError> {
    let options = if secrets::WIFI_PASSWORD.is_empty() {
        cyw43::JoinOptions::new_open()
    } else {
        let mut options = cyw43::JoinOptions::new(secrets::WIFI_PASSWORD.as_bytes());
        options.auth = auth;
        options
    };
    match with_timeout(Duration::from_millis(JOIN_TIMEOUT_MS), async {
        let mut ctrl = control.lock().await;
        ctrl.join(secrets::WIFI_SSID, options).await
    })
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(err)) => Err(JoinAttemptError::Join(err)),
        Err(_) => Err(JoinAttemptError::Timeout),
    }
}

/// Attempt to join the configured Wi-Fi network, bounded by a timeout.
///
/// WPA2-PSK is tried before the driver default (WPA2/WPA3), because the CYW43439
/// firmware drops a WPA3 (SAE) association every ~11 hours and then re-associates
/// on its own; every such disassociation costs a full rejoin. It also used to
/// cost an MCU reset: the no-IP countdown was stamped only at lease acquisition,
/// so the whole association age was charged to the outage that followed.
/// Re-arming the countdown at link loss removed that. See
/// `georgerobotics/cyw43-driver#161` and the Infineon report it cites.
///
/// The WPA2/WPA3 default is kept as a fallback so an access point that no longer
/// advertises WPA2-PSK (WPA3-only) still associates, at the cost of one extra
/// failed join attempt per reconnect on such a network.
async fn attempt_join(control: &'static ControlMutex) -> Result<(), JoinAttemptError> {
    let wpa2 = join_with(control, cyw43::JoinAuth::Wpa2).await;
    if wpa2.is_ok() || secrets::WIFI_PASSWORD.is_empty() {
        return wpa2;
    }

    defmt::warn!("wifi: WPA2 join failed; retrying with WPA2/WPA3");
    // No `leave()` here: `Control::join()` re-issues the full security
    // configuration, the timed-out attempt's future and lock guard are dropped
    // together, and the supervisor leaves on its next iteration.
    join_with(control, cyw43::JoinAuth::Wpa2Wpa3).await
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

/// Await DHCP configuration on the current association, then supervise the
/// active link. Returns when the link is lost, a rejoin is requested, or DHCP
/// never comes up.
async fn handle_associated(stack: NetStack, backoff: &mut Duration, watchdog: &mut NoIpWatchdog) {
    defmt::info!(
        "wifi: association successful to SSID: {}, awaiting DHCP lease...",
        secrets::WIFI_SSID
    );

    let mut attempts = 0;
    loop {
        crate::watchdog::note_wifi_alive();
        attempts += 1;
        // Note: do NOT race `wait_link_down()` here. The interface is not yet
        // reported "up" at this point, so that future would resolve instantly
        // and abort every association.
        match select(
            with_timeout(
                Duration::from_millis(DHCP_TIMEOUT_MS),
                stack.wait_config_up(),
            ),
            WIFI_REJOIN_SIGNAL.wait(),
        )
        .await
        {
            Either::First(Ok(())) => break,
            Either::First(Err(_)) => {
                if attempts >= DHCP_ATTEMPTS {
                    defmt::warn!("wifi: DHCP configuration timed out");
                    // Diagnostic code 3: DHCP timeout
                    beacon::BEACON_SIGNAL.signal(BeaconState::Error(3));
                    return;
                }
                // Keep the association and re-issue DHCP rather than paying a
                // full disassociate/rejoin for a slow lease.
                defmt::warn!("wifi: DHCP not up yet; retrying on the same association");
            }
            Either::Second(()) => {
                defmt::warn!("wifi: rejoin requested while awaiting DHCP");
                beacon::BEACON_SIGNAL.signal(BeaconState::Error(2));
                return;
            }
        }
    }

    log_ip_config(stack);
    let lease_start_ms = now_ms();
    beacon::BEACON_SIGNAL.signal(BeaconState::Connected);

    // Reset exponential backoff on successful connection.
    *backoff = Duration::from_millis(INITIAL_BACKOFF_MS);

    // Monitor the active link; return on loss or on a rejoin request, and only
    // refresh the no-IP watchdog once the lease has stayed up for STABLE_LEASE_MS.
    loop {
        crate::watchdog::note_wifi_alive();
        match with_timeout(
            Duration::from_millis(STABLE_LEASE_MS),
            select(stack.wait_link_down(), WIFI_REJOIN_SIGNAL.wait()),
        )
        .await
        {
            Ok(Either::First(())) => {
                watchdog.note_link_loss(lease_start_ms, now_ms());
                defmt::warn!("wifi: network link dropped!");
                beacon::BEACON_SIGNAL.signal(BeaconState::Error(2));
                break;
            }
            Ok(Either::Second(())) => {
                defmt::warn!("wifi: rejoin requested");
                beacon::BEACON_SIGNAL.signal(BeaconState::Error(2));
                break;
            }
            Err(_) => {
                // Lease has stayed up without link loss or rejoin request for STABLE_LEASE_MS.
                watchdog.note_last_good(now_ms());
            }
        }
    }
}

/// Leave the current association (bounded), clear any stale IPv4 lease and
/// reset the DHCPv4 state machine, and wait with exponential backoff before
/// the next attempt.
async fn leave_and_backoff(
    control: &'static ControlMutex,
    stack: NetStack,
    backoff: &mut Duration,
) {
    if with_timeout(Duration::from_millis(LEAVE_TIMEOUT_MS), async {
        let mut ctrl = control.lock().await;
        ctrl.leave().await;
    })
    .await
    .is_err()
    {
        defmt::warn!("wifi: leave did not complete; continuing");
    }

    stack.set_config_v4(embassy_net::ConfigV4::Dhcp(Default::default()));

    defmt::info!(
        "wifi: backing off for {}s before next retry...",
        backoff.as_secs()
    );
    Timer::after(*backoff).await;
    *backoff = core::cmp::min(*backoff * 2, Duration::from_millis(MAX_BACKOFF_MS));
}

/// Background Wi-Fi supervisor task.
///
/// Invariant: Wi-Fi connection loss, association failure, or DHCP timeout
/// never panic and never restart the MCU. Offline door control remains
/// completely unaffected. The single exception is the bounded no-IP reboot
/// escalation, which only fires after [`NO_IP_REBOOT_MS`] without a lease.
#[embassy_executor::task]
pub async fn wifi_supervisor_task(control: &'static ControlMutex, stack: NetStack) -> ! {
    let mut backoff = Duration::from_millis(INITIAL_BACKOFF_MS);
    let mut watchdog = NoIpWatchdog::new();

    loop {
        crate::watchdog::note_wifi_alive();
        defmt::info!(
            "wifi: initiating association to SSID: {}",
            secrets::WIFI_SSID
        );
        beacon::BEACON_SIGNAL.signal(BeaconState::Connecting);

        match attempt_join(control).await {
            Ok(()) => handle_associated(stack, &mut backoff, &mut watchdog).await,
            Err(JoinAttemptError::Timeout) => {
                defmt::warn!("wifi: association attempt timed out");
                beacon::BEACON_SIGNAL.signal(BeaconState::Error(1));
            }
            Err(JoinAttemptError::Join(err)) => {
                defmt::warn!("wifi: association failed: {:?}", defmt::Debug2Format(&err));
                beacon::BEACON_SIGNAL.signal(BeaconState::Error(1));
            }
        }

        if watchdog.reboot_due(now_ms()) && !crate::watchdog::is_ota_flashing() {
            defmt::error!(
                "wifi: no IP for {}s; resetting to recover the radio",
                NO_IP_REBOOT_MS / 1000
            );
            crate::watchdog::trigger_reset();
        }

        leave_and_backoff(control, stack, &mut backoff).await;
    }
}
