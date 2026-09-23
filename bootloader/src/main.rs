//! Standalone A/B bootloader for garagedoorcontroller.
//!
//! Inspects the `embassy-boot` state partition, swaps the DFU image into ACTIVE
//! when a swap is pending, and boots the ACTIVE partition.

#![no_std]
#![no_main]

use core::alloc::{GlobalAlloc, Layout};
use core::cell::RefCell;

use cortex_m_rt::{entry, exception};
use defmt_rtt as _;
use embassy_boot_rp::{BootLoader, BootLoaderConfig, State, WatchdogFlash};
use embassy_sync::blocking_mutex::Mutex;
use embassy_time::Duration;

const FLASH_SIZE: usize = garagedoor_core::flash::FLASH_BYTES as usize;

/// Fail-fast global allocator.
/// Satisfies the linker if any dependency pulls alloc via workspace feature unification.
struct NoAlloc;

unsafe impl GlobalAlloc for NoAlloc {
    unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
        core::ptr::null_mut()
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}
}

#[global_allocator]
static NO_ALLOC: NoAlloc = NoAlloc;

#[entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());

    let flash = WatchdogFlash::<FLASH_SIZE>::start(p.FLASH, p.WATCHDOG, Duration::from_secs(8));
    let flash = Mutex::new(RefCell::new(flash));

    let config = BootLoaderConfig::from_linkerfile_blocking(&flash, &flash, &flash);
    let active_offset = config.active.offset();
    let bl: BootLoader = BootLoader::prepare(config);

    if bl.state == State::DfuDetach {
        defmt::info!("bootloader: DFU detach requested, resetting to USB boot");
        embassy_rp::rom_data::reset_to_usb_boot(0, 0);
    }

    defmt::info!(
        "bootloader: loading active image at offset 0x{:x}",
        active_offset
    );

    unsafe { bl.load(embassy_rp::flash::FLASH_BASE as u32 + active_offset) }
}

#[no_mangle]
#[cfg_attr(target_os = "none", link_section = ".HardFault.user")]
unsafe extern "C" fn HardFault() {
    cortex_m::peripheral::SCB::sys_reset();
}

#[exception]
unsafe fn DefaultHandler(_: i16) -> ! {
    const SCB_ICSR: *const u32 = 0xE000_ED04 as *const u32;
    let irqn = unsafe { core::ptr::read_volatile(SCB_ICSR) } as u8 as i16 - 16;

    panic!("DefaultHandler #{:?}", irqn);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    cortex_m::asm::udf()
}
