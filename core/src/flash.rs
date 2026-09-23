//! Flash memory layout and partition definitions for RP2040 NOR flash.
//!
//! Defines canonical physical and partition constants, compile-time layout
//! invariants, and pure helper functions for address arithmetic and partition checks.

/// Physical base address of memory-mapped flash on RP2040 (XIP window).
pub const FLASH_BASE: u32 = 0x1000_0000;
/// Total size of on-board QSPI NOR flash (2 MiB = 2,097,152 bytes).
pub const FLASH_BYTES: u32 = 2 * 1024 * 1024;
/// One byte past the end of physical flash (0x1020_0000).
pub const FLASH_END: u32 = FLASH_BASE + FLASH_BYTES;

/// Erase sector/page size of RP2040 flash (4 KiB = 4,096 bytes).
pub const PAGE_BYTES: u32 = 4096;
/// Minimum flash write unit in bytes for RP2040 NOR flash.
pub const WRITE_BYTES: u32 = 1;

/// Base address of the bootloader partition (contains Boot2 and embassy-boot-rp).
pub const BOOTLOADER_BASE: u32 = 0x1000_0000;
/// Total size of the bootloader partition (24 KiB = 24,576 bytes).
pub const BOOTLOADER_BYTES: u32 = 24 * 1024;

/// Base address of the active application partition.
pub const ACTIVE_BASE: u32 = 0x1000_6000;
/// Total size of the active application partition (780 KiB = 798,720 bytes).
pub const ACTIVE_BYTES: u32 = 780 * 1024;

/// Base address of the DFU staging partition.
pub const DFU_BASE: u32 = 0x100C_9000;
/// Total size of the DFU staging partition (784 KiB = 802,816 bytes).
pub const DFU_BYTES: u32 = 784 * 1024;

/// Base address of the embassy-boot swap state partition.
pub const STATE_BASE: u32 = 0x1018_D000;
/// Total size of the state partition (4 KiB = 4,096 bytes).
pub const STATE_BYTES: u32 = 4 * 1024;

/// Base address of reserved spare flash.
pub const SPARE_BASE: u32 = 0x1018_E000;
/// Total size of reserved spare flash (456 KiB = 466,944 bytes).
pub const SPARE_BYTES: u32 = FLASH_END - SPARE_BASE;

/// Boot2 second-stage bootloader sector size in bytes at start of flash.
pub const BOOT2_BYTES: u32 = 256;

const _: () = {
    assert!(BOOTLOADER_BASE.is_multiple_of(PAGE_BYTES));
    assert!(ACTIVE_BASE.is_multiple_of(PAGE_BYTES));
    assert!(DFU_BASE.is_multiple_of(PAGE_BYTES));
    assert!(STATE_BASE.is_multiple_of(PAGE_BYTES));
    assert!(SPARE_BASE.is_multiple_of(PAGE_BYTES));

    assert!(ACTIVE_BYTES.is_multiple_of(PAGE_BYTES));
    assert!(DFU_BYTES.is_multiple_of(PAGE_BYTES));
    assert!(STATE_BYTES.is_multiple_of(PAGE_BYTES));
    assert!(SPARE_BYTES.is_multiple_of(PAGE_BYTES));

    assert!(BOOTLOADER_BASE + BOOTLOADER_BYTES == ACTIVE_BASE);
    assert!(DFU_BASE == ACTIVE_BASE + ACTIVE_BYTES);
    assert!(DFU_BYTES == ACTIVE_BYTES + PAGE_BYTES);
    assert!(STATE_BASE == DFU_BASE + DFU_BYTES);
    assert!(SPARE_BASE == STATE_BASE + STATE_BYTES);
    assert!(SPARE_BASE + SPARE_BYTES == FLASH_END);

    assert!(2 + 4 * (ACTIVE_BYTES / PAGE_BYTES) <= STATE_BYTES / WRITE_BYTES);
};

/// Canonical partitions within the 2 MiB flash memory map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partition {
    /// Bootloader partition (Boot2 + embassy-boot-rp binary).
    Bootloader,
    /// Active running application slot.
    Active,
    /// DFU staging slot for incoming firmware updates.
    Dfu,
    /// Swap transaction state tracking partition.
    State,
    /// Reserved/spare flash capacity.
    Spare,
}

impl Partition {
    /// Physical base address of this partition in memory-mapped XIP space.
    pub const fn base_address(self) -> u32 {
        match self {
            Self::Bootloader => BOOTLOADER_BASE,
            Self::Active => ACTIVE_BASE,
            Self::Dfu => DFU_BASE,
            Self::State => STATE_BASE,
            Self::Spare => SPARE_BASE,
        }
    }

    /// Flash-relative start offset of this partition (from `0x0000_0000`).
    pub const fn offset(self) -> u32 {
        self.base_address() - FLASH_BASE
    }

    /// Size of this partition in bytes.
    pub const fn len_bytes(self) -> u32 {
        match self {
            Self::Bootloader => BOOTLOADER_BYTES,
            Self::Active => ACTIVE_BYTES,
            Self::Dfu => DFU_BYTES,
            Self::State => STATE_BYTES,
            Self::Spare => SPARE_BYTES,
        }
    }

    /// Physical end address (exclusive upper bound) of this partition in memory-mapped XIP space.
    pub const fn end_address(self) -> u32 {
        self.base_address() + self.len_bytes()
    }

    /// Flash-relative end offset (exclusive upper bound) of this partition.
    pub const fn end_offset(self) -> u32 {
        self.offset() + self.len_bytes()
    }

    /// Number of 4 KiB flash pages spanning this partition.
    pub const fn page_count(self) -> u32 {
        self.len_bytes() / PAGE_BYTES
    }

    /// Returns `true` if the memory-mapped CPU address lies within this partition.
    pub const fn contains_address(self, addr: u32) -> bool {
        addr >= self.base_address() && addr < self.end_address()
    }

    /// Returns `true` if the flash-relative offset lies within this partition.
    pub const fn contains_offset(self, offset: u32) -> bool {
        offset >= self.offset() && offset < self.end_offset()
    }
}

/// Converts a flash-relative offset (0..2 MiB) to a CPU memory-mapped address.
///
/// Returns `Some(address)` if within flash bounds, or `None` if offset exceeds flash size.
pub const fn flash_offset_to_address(offset: u32) -> Option<u32> {
    if offset < FLASH_BYTES {
        Some(FLASH_BASE + offset)
    } else {
        None
    }
}

/// Converts a CPU memory-mapped address (0x1000_0000..0x1020_0000) to a flash-relative offset.
///
/// Returns `Some(offset)` if address lies in flash XIP window, or `None` otherwise.
pub const fn address_to_flash_offset(addr: u32) -> Option<u32> {
    if addr >= FLASH_BASE && addr < FLASH_END {
        Some(addr - FLASH_BASE)
    } else {
        None
    }
}

/// Returns `true` if the given address or offset is aligned to a 4 KiB flash page boundary.
pub const fn is_page_aligned(addr_or_offset: u32) -> bool {
    addr_or_offset.is_multiple_of(PAGE_BYTES)
}

/// Finds the partition containing the given CPU memory-mapped address.
pub const fn partition_for_address(addr: u32) -> Option<Partition> {
    if Partition::Bootloader.contains_address(addr) {
        Some(Partition::Bootloader)
    } else if Partition::Active.contains_address(addr) {
        Some(Partition::Active)
    } else if Partition::Dfu.contains_address(addr) {
        Some(Partition::Dfu)
    } else if Partition::State.contains_address(addr) {
        Some(Partition::State)
    } else if Partition::Spare.contains_address(addr) {
        Some(Partition::Spare)
    } else {
        None
    }
}

/// Finds the partition containing the given flash-relative offset.
pub const fn partition_for_offset(offset: u32) -> Option<Partition> {
    if Partition::Bootloader.contains_offset(offset) {
        Some(Partition::Bootloader)
    } else if Partition::Active.contains_offset(offset) {
        Some(Partition::Active)
    } else if Partition::Dfu.contains_offset(offset) {
        Some(Partition::Dfu)
    } else if Partition::State.contains_offset(offset) {
        Some(Partition::State)
    } else if Partition::Spare.contains_offset(offset) {
        Some(Partition::Spare)
    } else {
        None
    }
}

/// Returns the number of 4 KiB flash pages in the active partition (195).
pub const fn active_page_count() -> u32 {
    ACTIVE_BYTES / PAGE_BYTES
}

/// Returns the number of 4 KiB flash pages in the DFU staging partition (196).
pub const fn dfu_page_count() -> u32 {
    DFU_BYTES / PAGE_BYTES
}

/// Computes the required state write units for embassy-boot swap transaction algorithm.
///
/// Formula: `2 + 4 * (active_bytes / PAGE_BYTES)`.
pub const fn required_state_write_units(active_bytes: u32) -> u32 {
    2 + 4 * (active_bytes / PAGE_BYTES)
}

/// Available write units in the STATE partition (4,096 units).
pub const fn state_write_units() -> u32 {
    STATE_BYTES / WRITE_BYTES
}

/// Maximum permissible application binary size in bytes (780 KiB = 798,720 bytes).
pub const fn max_app_binary_bytes() -> u32 {
    ACTIVE_BYTES
}

/// Maximum permissible bootloader binary size in bytes (24 KiB - 256 B Boot2 = 24,320 bytes).
pub const fn max_bootloader_binary_bytes() -> u32 {
    BOOTLOADER_BYTES - BOOT2_BYTES
}

/// Returns `true` if a firmware binary of `binary_size` bytes fits inside the active application slot.
pub const fn can_fit_in_active(binary_size: u32) -> bool {
    binary_size <= ACTIVE_BYTES
}

/// Returns `true` if a staging binary of `binary_size` bytes fits inside the DFU slot.
pub const fn can_fit_in_dfu(binary_size: u32) -> bool {
    binary_size <= DFU_BYTES
}

/// Returns `true` if a bootloader binary fits within the bootloader partition after Boot2.
pub const fn can_fit_in_bootloader(binary_size: u32) -> bool {
    binary_size <= max_bootloader_binary_bytes()
}

/// Lifecycle states for embassy-boot firmware update transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    /// Normal operation: running confirmed active image.
    Boot,
    /// Staged update written to DFU partition and marked for swap.
    SwapPending,
    /// Device booted after swap into trial active image, awaiting confirmation.
    SwapTesting,
    /// Unconfirmed image failed or reset before confirmation; bootloader rolled back.
    Revert,
    /// Recovery escape: application commanded reboot into ROM USB bootloader.
    DfuDetach,
}

/// Action taken by the bootloader upon device reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    /// Boot active partition directly.
    BootActive,
    /// Partition swap completed; trial image booted.
    SwappedAndBoot,
    /// Rollback completed; previous stable image restored and booted.
    RolledBackAndBoot,
    /// Reset to USB ROM bootloader requested.
    EnterUsbBoot,
}

/// State machine modeling the embassy-boot dual-bank update, swap, rollback, and confirmation lifecycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStateMachine {
    state: UpdateState,
    active_version: u32,
    staged_version: Option<u32>,
    previous_version: Option<u32>,
}

impl UpdateStateMachine {
    /// Initializes a new state machine running `initial_version` in the confirmed `Boot` state.
    pub const fn new(initial_version: u32) -> Self {
        Self {
            state: UpdateState::Boot,
            active_version: initial_version,
            staged_version: None,
            previous_version: None,
        }
    }

    /// Current update lifecycle state.
    pub const fn state(&self) -> UpdateState {
        self.state
    }

    /// Currently executing active image version.
    pub const fn active_version(&self) -> u32 {
        self.active_version
    }

    /// Staged DFU image version, if present.
    pub const fn staged_version(&self) -> Option<u32> {
        self.staged_version
    }

    /// Stage a new firmware binary version into the DFU partition.
    pub fn stage_firmware(&mut self, version: u32, size_bytes: u32) -> Result<(), &'static str> {
        if !can_fit_in_dfu(size_bytes) {
            return Err("firmware binary exceeds DFU partition capacity");
        }
        self.staged_version = Some(version);
        Ok(())
    }

    /// Command a partition swap on the next boot (corresponds to `Updater::mark_updated()`).
    pub fn mark_updated(&mut self) -> Result<(), &'static str> {
        if self.staged_version.is_none() {
            return Err("cannot mark updated without staged firmware");
        }
        if self.state == UpdateState::SwapTesting {
            return Err("cannot mark updated while unconfirmed swap is pending verification");
        }
        self.state = UpdateState::SwapPending;
        Ok(())
    }

    /// Permanently confirm and commit the running image (corresponds to `Updater::mark_booted()`).
    pub fn mark_booted(&mut self) -> Result<(), &'static str> {
        match self.state {
            UpdateState::SwapTesting | UpdateState::Boot | UpdateState::Revert => {
                self.state = UpdateState::Boot;
                self.previous_version = None;
                self.staged_version = None;
                Ok(())
            }
            UpdateState::SwapPending => {
                Err("cannot mark booted while update is pending reboot swap")
            }
            UpdateState::DfuDetach => Err("cannot mark booted in DFU detach state"),
        }
    }

    /// Request USB DFU mode on the next reset (corresponds to `Updater::mark_dfu()`).
    pub fn mark_dfu(&mut self) {
        self.state = UpdateState::DfuDetach;
    }

    /// Simulates a system reset and bootloader execution.
    pub fn on_reset(&mut self) -> BootAction {
        match self.state {
            UpdateState::Boot => BootAction::BootActive,
            UpdateState::SwapPending => {
                // Bootloader executes partition swap
                let new_active = self
                    .staged_version
                    .take()
                    .expect("staged version must exist");
                self.previous_version = Some(self.active_version);
                self.active_version = new_active;
                self.state = UpdateState::SwapTesting;
                BootAction::SwappedAndBoot
            }
            UpdateState::SwapTesting => {
                // Unconfirmed reset! Automatic rollback to previous version
                let rollback_ver = self
                    .previous_version
                    .take()
                    .expect("previous version must exist");
                self.staged_version = Some(self.active_version);
                self.active_version = rollback_ver;
                self.state = UpdateState::Revert;
                BootAction::RolledBackAndBoot
            }
            UpdateState::Revert => BootAction::BootActive,
            UpdateState::DfuDetach => BootAction::EnterUsbBoot,
        }
    }
}

#[cfg(test)]
mod tests;
