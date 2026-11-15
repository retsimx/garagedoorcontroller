//! Hardware watchdog ownership.
//!
//! This module owns the single `WATCHDOG` peripheral. It is armed with an 8 s
//! timeout as the first action in `main`, before any driver is initialised, and
//! is fed both by [`feeder_task`] (every 500 ms) and by every blocking flash
//! operation in [`crate::update`]. A hang anywhere in the boot or self-test path
//! therefore stops the feed and lets the hardware reset the MCU, which is what
//! makes the bootloader revert an unconfirmed image.

use core::cell::RefCell;

use embassy_rp::peripherals::WATCHDOG;
use embassy_rp::watchdog::{ResetReason, Watchdog};
use embassy_rp::Peri;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_time::{Duration, Instant, Timer};
use garagedoor_core::wifi::task_stalled;
use portable_atomic::{AtomicBool, AtomicU32, Ordering};

/// Watchdog timeout. An image that stops feeding is reset after 8 s, letting the
/// bootloader revert an unconfirmed swap.
pub const TIMEOUT: Duration = Duration::from_secs(8);

/// Period at which [`feeder_task`] reloads the watchdog.
const FEED_PERIOD: Duration = Duration::from_millis(500);

static WATCHDOG: BlockingMutex<CriticalSectionRawMutex, RefCell<Option<Watchdog>>> =
    BlockingMutex::new(RefCell::new(None));

static SUPERVISION_ARMED: AtomicBool = AtomicBool::new(false);
static OTA_FLASHING: AtomicBool = AtomicBool::new(false);
static WIFI_HEARTBEAT_S: AtomicU32 = AtomicU32::new(0);
static TELEMETRY_HEARTBEAT_S: AtomicU32 = AtomicU32::new(0);

fn now_s() -> u32 {
    Instant::now().as_secs() as u32
}

/// Record liveness progress from [`crate::wifi::wifi_supervisor_task`].
pub fn note_wifi_alive() {
    WIFI_HEARTBEAT_S.store(now_s(), Ordering::Relaxed);
}

/// Record liveness progress from [`crate::telemetry::telemetry_task`].
pub fn note_telemetry_alive() {
    TELEMETRY_HEARTBEAT_S.store(now_s(), Ordering::Relaxed);
}

/// Arm supervisor task stall detection once both network supervisor tasks are spawned.
pub fn arm_task_supervision() {
    let now = now_s();
    WIFI_HEARTBEAT_S.store(now, Ordering::Relaxed);
    TELEMETRY_HEARTBEAT_S.store(now, Ordering::Relaxed);
    SUPERVISION_ARMED.store(true, Ordering::Relaxed);
}

/// RAII guard that suppresses soft network-health resets and supervisor stall
/// checks while streaming an OTA image to flash.
pub struct OtaFlashGuard;

/// Mark an OTA flash write session active; cleared automatically on drop.
pub fn enter_ota_flash() -> OtaFlashGuard {
    OTA_FLASHING.store(true, Ordering::Relaxed);
    OtaFlashGuard
}

impl Drop for OtaFlashGuard {
    fn drop(&mut self) {
        OTA_FLASHING.store(false, Ordering::Relaxed);
        note_wifi_alive();
        note_telemetry_alive();
    }
}

/// Returns `true` while an OTA flash write session is in progress.
pub fn is_ota_flashing() -> bool {
    OTA_FLASHING.load(Ordering::Relaxed)
}

/// Read the reset reason, log it, arm the 8 s timeout, and install the shared
/// handle. Call once, as the first action in `main`.
pub fn init(peripheral: Peri<'static, WATCHDOG>) {
    let mut wd = Watchdog::new(peripheral);
    let reset_reason = match wd.reset_reason() {
        Some(ResetReason::Forced) => "watchdog-forced",
        Some(ResetReason::TimedOut) => "watchdog-timeout",
        None => "power-on-or-debugger",
    };
    defmt::info!("reset_reason={}", reset_reason);
    wd.start(TIMEOUT);
    WATCHDOG.lock(|cell| *cell.borrow_mut() = Some(wd));
}

/// Reload the shared watchdog if it has been installed. Called by the feeder
/// task and by every `DfuFlash` erase/write/read.
pub fn feed() {
    WATCHDOG.lock(|cell| {
        if let Some(wd) = cell.borrow_mut().as_mut() {
            wd.feed(TIMEOUT);
        }
    });
}

/// Trigger an immediate hardware watchdog reset (preferring RP2040 `PSM_WDSEL`
/// full-subsystem reset over Cortex-M `AIRCR.SYSRESETREQ`).
pub fn trigger_reset() -> ! {
    WATCHDOG.lock(|cell| {
        if let Some(wd) = cell.borrow_mut().as_mut() {
            wd.trigger_reset();
        }
    });
    cortex_m::peripheral::SCB::sys_reset();
}

/// Reload the watchdog every 500 ms for the life of the program, unless a
/// supervised network task has stalled beyond [`garagedoor_core::wifi::TASK_STALL_TIMEOUT_MS`].
#[embassy_executor::task]
pub async fn feeder_task() -> ! {
    loop {
        if SUPERVISION_ARMED.load(Ordering::Relaxed) && !is_ota_flashing() {
            let now_ms = (now_s() as u64) * 1000;
            let wifi_last_ms = (WIFI_HEARTBEAT_S.load(Ordering::Relaxed) as u64) * 1000;
            let telemetry_last_ms = (TELEMETRY_HEARTBEAT_S.load(Ordering::Relaxed) as u64) * 1000;
            if task_stalled(wifi_last_ms, now_ms) || task_stalled(telemetry_last_ms, now_ms) {
                defmt::error!("watchdog: supervisor task stalled; withholding feed");
                Timer::after(FEED_PERIOD).await;
                continue;
            }
        }
        feed();
        Timer::after(FEED_PERIOD).await;
    }
}
