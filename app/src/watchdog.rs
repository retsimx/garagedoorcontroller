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
use embassy_time::{Duration, Timer};

/// Watchdog timeout. An image that stops feeding is reset after 8 s, letting the
/// bootloader revert an unconfirmed swap.
pub const TIMEOUT: Duration = Duration::from_secs(8);

/// Period at which [`feeder_task`] reloads the watchdog.
const FEED_PERIOD: Duration = Duration::from_millis(500);

static WATCHDOG: BlockingMutex<CriticalSectionRawMutex, RefCell<Option<Watchdog>>> =
    BlockingMutex::new(RefCell::new(None));

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

/// Reload the watchdog every 500 ms for the life of the program.
#[embassy_executor::task]
pub async fn feeder_task() -> ! {
    loop {
        feed();
        Timer::after(FEED_PERIOD).await;
    }
}
