use super::*;

#[test]
fn test_partition_constants_and_sizes() {
    assert_eq!(FLASH_BASE, 0x1000_0000);
    assert_eq!(FLASH_BYTES, 2 * 1024 * 1024);
    assert_eq!(FLASH_END, FLASH_BASE + FLASH_BYTES);
    assert_eq!(PAGE_BYTES, 4096);
    assert_eq!(WRITE_BYTES, 1);

    assert_eq!(BOOTLOADER_BYTES, 24 * 1024);
    assert_eq!(ACTIVE_BYTES, 780 * 1024);
    assert_eq!(DFU_BYTES, 784 * 1024);
    assert_eq!(STATE_BYTES, 4 * 1024);
    assert_eq!(SPARE_BYTES, 456 * 1024);

    assert_eq!(
        BOOTLOADER_BYTES + ACTIVE_BYTES + DFU_BYTES + STATE_BYTES + SPARE_BYTES,
        FLASH_BYTES
    );
}

#[test]
fn test_partition_contiguity_and_offsets() {
    assert_eq!(BOOTLOADER_BASE, 0x1000_0000);
    assert_eq!(BOOTLOADER_BASE + BOOTLOADER_BYTES, ACTIVE_BASE);
    assert_eq!(ACTIVE_BASE, 0x1000_6000);
    assert_eq!(ACTIVE_BASE + ACTIVE_BYTES, DFU_BASE);
    assert_eq!(DFU_BASE, 0x100C_9000);
    assert_eq!(DFU_BASE + DFU_BYTES, STATE_BASE);
    assert_eq!(STATE_BASE, 0x1018_D000);
    assert_eq!(STATE_BASE + STATE_BYTES, SPARE_BASE);
    assert_eq!(SPARE_BASE, 0x1018_E000);
    assert_eq!(SPARE_BASE + SPARE_BYTES, FLASH_END);

    assert_eq!(Partition::Bootloader.offset(), 0x0000_0000);
    assert_eq!(Partition::Active.offset(), 0x0000_6000);
    assert_eq!(Partition::Dfu.offset(), 0x000C_9000);
    assert_eq!(Partition::State.offset(), 0x0018_D000);
    assert_eq!(Partition::Spare.offset(), 0x0018_E000);
}

#[test]
fn test_page_alignment() {
    for base in [
        BOOTLOADER_BASE,
        ACTIVE_BASE,
        DFU_BASE,
        STATE_BASE,
        SPARE_BASE,
        FLASH_END,
    ] {
        assert!(is_page_aligned(base), "base {base:#x} must be page aligned");
    }

    for len in [
        BOOTLOADER_BYTES,
        ACTIVE_BYTES,
        DFU_BYTES,
        STATE_BYTES,
        SPARE_BYTES,
        FLASH_BYTES,
    ] {
        assert!(is_page_aligned(len), "length {len:#x} must be page aligned");
    }

    assert!(!is_page_aligned(1));
    assert!(!is_page_aligned(PAGE_BYTES - 1));
    assert!(is_page_aligned(PAGE_BYTES));
    assert!(!is_page_aligned(PAGE_BYTES + 1));
}

#[test]
fn test_dfu_active_page_relation() {
    assert_eq!(DFU_BYTES, ACTIVE_BYTES + PAGE_BYTES);
    assert_eq!(active_page_count(), 195);
    assert_eq!(dfu_page_count(), 196);
    assert_eq!(dfu_page_count(), active_page_count() + 1);
    assert_eq!(Partition::Active.page_count(), 195);
    assert_eq!(Partition::Dfu.page_count(), 196);
}

#[test]
fn test_state_capacity_formula() {
    let req_units = required_state_write_units(ACTIVE_BYTES);
    assert_eq!(req_units, 2 + 4 * 195);
    assert_eq!(req_units, 782);

    let avail_units = state_write_units();
    assert_eq!(avail_units, 4096);
    assert!(avail_units >= req_units);

    // Safety margin > 5x
    assert!(avail_units > req_units * 5);
}

#[test]
fn test_address_and_offset_conversions() {
    // Flash offset to address
    assert_eq!(flash_offset_to_address(0), Some(FLASH_BASE));
    assert_eq!(
        flash_offset_to_address(FLASH_BYTES - 1),
        Some(FLASH_END - 1)
    );
    assert_eq!(flash_offset_to_address(FLASH_BYTES), None);
    assert_eq!(flash_offset_to_address(FLASH_BYTES + 1), None);
    assert_eq!(flash_offset_to_address(u32::MAX), None);

    // Address to flash offset
    assert_eq!(address_to_flash_offset(FLASH_BASE), Some(0));
    assert_eq!(
        address_to_flash_offset(FLASH_END - 1),
        Some(FLASH_BYTES - 1)
    );
    assert_eq!(address_to_flash_offset(FLASH_BASE - 1), None);
    assert_eq!(address_to_flash_offset(FLASH_END), None);
    assert_eq!(address_to_flash_offset(0), None);
    assert_eq!(address_to_flash_offset(u32::MAX), None);

    // Roundtrip conversions
    for offset in [
        0,
        0x6000,
        0xC_9000,
        0x18_D000,
        0x18_E000,
        FLASH_BYTES / 2,
        FLASH_BYTES - 1,
    ] {
        let addr = flash_offset_to_address(offset).expect("valid offset");
        let roundtrip = address_to_flash_offset(addr).expect("valid address");
        assert_eq!(roundtrip, offset);
    }
}

#[test]
fn test_partition_containment_and_lookup() {
    let partitions = [
        (Partition::Bootloader, BOOTLOADER_BASE, BOOTLOADER_BYTES),
        (Partition::Active, ACTIVE_BASE, ACTIVE_BYTES),
        (Partition::Dfu, DFU_BASE, DFU_BYTES),
        (Partition::State, STATE_BASE, STATE_BYTES),
        (Partition::Spare, SPARE_BASE, SPARE_BYTES),
    ];

    for (part, base, len) in partitions {
        assert_eq!(part.base_address(), base);
        assert_eq!(part.len_bytes(), len);
        assert_eq!(part.end_address(), base + len);
        assert_eq!(part.offset(), base - FLASH_BASE);
        assert_eq!(part.end_offset(), (base - FLASH_BASE) + len);

        // Boundary checks: base should be inside, end should NOT be inside
        assert!(part.contains_address(base));
        assert!(part.contains_address(base + len / 2));
        assert!(part.contains_address(base + len - 1));
        assert!(!part.contains_address(base + len));
        if base > FLASH_BASE {
            assert!(!part.contains_address(base - 1));
        }

        // Offset boundary checks
        let off = base - FLASH_BASE;
        assert!(part.contains_offset(off));
        assert!(part.contains_offset(off + len / 2));
        assert!(part.contains_offset(off + len - 1));
        assert!(!part.contains_offset(off + len));
        if off > 0 {
            assert!(!part.contains_offset(off - 1));
        }

        // Lookup checks
        assert_eq!(partition_for_address(base), Some(part));
        assert_eq!(partition_for_address(base + len / 2), Some(part));
        assert_eq!(partition_for_address(base + len - 1), Some(part));

        assert_eq!(partition_for_offset(off), Some(part));
        assert_eq!(partition_for_offset(off + len / 2), Some(part));
        assert_eq!(partition_for_offset(off + len - 1), Some(part));
    }

    // Out of bounds lookups
    assert_eq!(partition_for_address(FLASH_BASE - 1), None);
    assert_eq!(partition_for_address(FLASH_END), None);
    assert_eq!(partition_for_address(0), None);
    assert_eq!(partition_for_address(u32::MAX), None);

    assert_eq!(partition_for_offset(FLASH_BYTES), None);
    assert_eq!(partition_for_offset(u32::MAX), None);
}

#[test]
fn test_capacity_checks() {
    assert_eq!(max_app_binary_bytes(), ACTIVE_BYTES);
    assert_eq!(max_app_binary_bytes(), 798_720);
    assert!(can_fit_in_active(0));
    assert!(can_fit_in_active(ACTIVE_BYTES));
    assert!(!can_fit_in_active(ACTIVE_BYTES + 1));

    assert!(can_fit_in_dfu(0));
    assert!(can_fit_in_dfu(DFU_BYTES));
    assert!(!can_fit_in_dfu(DFU_BYTES + 1));

    assert_eq!(BOOT2_BYTES, 256);
    assert_eq!(max_bootloader_binary_bytes(), 24 * 1024 - 256);
    assert_eq!(max_bootloader_binary_bytes(), 24_320);
    assert!(can_fit_in_bootloader(0));
    assert!(can_fit_in_bootloader(24_320));
    assert!(!can_fit_in_bootloader(24_321));
}

#[test]
fn test_state_machine_successful_update_lifecycle() {
    let mut sm = UpdateStateMachine::new(1);
    assert_eq!(sm.state(), UpdateState::Boot);
    assert_eq!(sm.active_version(), 1);
    assert_eq!(sm.staged_version(), None);

    // Reset while Boot boots active without swap
    assert_eq!(sm.on_reset(), BootAction::BootActive);
    assert_eq!(sm.active_version(), 1);

    // Stage valid firmware (version 2, 350 KiB)
    assert!(sm.stage_firmware(2, 350 * 1024).is_ok());
    assert_eq!(sm.staged_version(), Some(2));

    // Mark updated: schedules swap on reset
    assert!(sm.mark_updated().is_ok());
    assert_eq!(sm.state(), UpdateState::SwapPending);

    // Reset occurs: bootloader swaps partition and boots new trial image
    assert_eq!(sm.on_reset(), BootAction::SwappedAndBoot);
    assert_eq!(sm.state(), UpdateState::SwapTesting);
    assert_eq!(sm.active_version(), 2);

    // Trial image confirms itself
    assert!(sm.mark_booted().is_ok());
    assert_eq!(sm.state(), UpdateState::Boot);

    // Subsequent resets stay in confirmed Boot running version 2
    assert_eq!(sm.on_reset(), BootAction::BootActive);
    assert_eq!(sm.active_version(), 2);
}

#[test]
fn test_state_machine_rollback_on_unconfirmed_crash() {
    let mut sm = UpdateStateMachine::new(1);

    // Stage version 2 and schedule swap
    assert!(sm.stage_firmware(2, 400 * 1024).is_ok());
    assert!(sm.mark_updated().is_ok());
    assert_eq!(sm.state(), UpdateState::SwapPending);

    // Reset occurs: swap executed, trial image running
    assert_eq!(sm.on_reset(), BootAction::SwappedAndBoot);
    assert_eq!(sm.state(), UpdateState::SwapTesting);
    assert_eq!(sm.active_version(), 2);

    // Version 2 crashes / resets BEFORE mark_booted() is called
    // Bootloader detects unconfirmed SwapTesting state and rolls back
    assert_eq!(sm.on_reset(), BootAction::RolledBackAndBoot);
    assert_eq!(sm.state(), UpdateState::Revert);
    assert_eq!(sm.active_version(), 1);

    // Subsequent reboot boots stable version 1
    assert_eq!(sm.on_reset(), BootAction::BootActive);
    assert_eq!(sm.active_version(), 1);
}

#[test]
fn test_state_machine_dfu_detach_recovery() {
    let mut sm = UpdateStateMachine::new(1);
    sm.mark_dfu();
    assert_eq!(sm.state(), UpdateState::DfuDetach);

    // On reset, bootloader enters USB ROM bootloader instead of loading active
    assert_eq!(sm.on_reset(), BootAction::EnterUsbBoot);
}

#[test]
fn test_state_machine_validation_guards() {
    let mut sm = UpdateStateMachine::new(1);

    // Cannot mark updated without staged firmware
    assert!(sm.mark_updated().is_err());

    // Reject firmware exceeding active partition capacity
    assert!(sm.stage_firmware(2, ACTIVE_BYTES + 1).is_err());

    // Stage valid firmware and mark updated
    assert!(sm.stage_firmware(2, 100 * 1024).is_ok());
    assert!(sm.mark_updated().is_ok());

    // Cannot mark booted while swap is pending reboot
    assert!(sm.mark_booted().is_err());

    // Execute swap
    assert_eq!(sm.on_reset(), BootAction::SwappedAndBoot);

    // Cannot mark updated while already in SwapTesting
    assert!(sm.mark_updated().is_err());

    // Confirm
    assert!(sm.mark_booted().is_ok());
    assert_eq!(sm.state(), UpdateState::Boot);
}
