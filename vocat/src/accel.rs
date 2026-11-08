//! What Whistle needs from the chip that esp-hal doesn't provide: the model
//! partition mapped into the data bus, and the PIE vector unit switched on.

/// Flash offset of the `model` partition (partitions.csv).
pub const MODEL_FLASH_OFFSET: u32 = 0x21_0000;
/// Size of the `model` partition.
pub const MODEL_PARTITION_SIZE: usize = 0xDF_0000;

const PAGE: usize = 64 * 1024;
/// End of the ESP32-S3 external-memory data bus window (0x3C00_0000..0x3E00_0000).
const DBUS_END: usize = 0x3E00_0000;
const DBUS_START: usize = 0x3C00_0000;
/// Entries in the MMU table shared by IBUS and DBUS; the bootloader uses the last.
const MMU_ENTRIES: usize = 512;
const MMU_ACCESS_FLASH: u32 = 0;

#[derive(Debug)]
#[allow(dead_code, reason = "read through Debug in the log")]
pub enum MapError {
    /// Not enough data-bus address space after PSRAM.
    NoAddressSpace { vaddr: usize, len: usize },
    /// The ROM MMU routine refused (code).
    Rom(i32),
}

/// Maps `len` bytes of flash from `flash_offset` into the data bus at the
/// first page after `after` (the end of the PSRAM mapping), read-only through
/// the data cache, the way esp-hal maps PSRAM.
///
/// # Safety
/// Call once, before anything else changes the MMU. The returned memory is
/// whatever is in flash; the caller validates it.
#[esp_hal::ram]
pub unsafe fn map_flash(after: usize, flash_offset: u32, len: usize) -> Result<&'static [u8], MapError> {
    unsafe extern "C" {
        fn Cache_Suspend_DCache() -> u32;
        fn Cache_Resume_DCache(param: u32);
        fn cache_dbus_mmu_set(ext_ram: u32, vaddr: u32, paddr: u32, psize: u32, num: u32, fixed: u32) -> i32;
    }
    let vaddr = after.next_multiple_of(PAGE);
    let pages = len.div_ceil(PAGE);
    let last_entry = (vaddr - DBUS_START) / PAGE + pages;
    if vaddr + pages * PAGE > DBUS_END || last_entry >= MMU_ENTRIES - 1 {
        return Err(MapError::NoAddressSpace { vaddr, len });
    }
    let rc = unsafe {
        Cache_Suspend_DCache();
        let rc = cache_dbus_mmu_set(MMU_ACCESS_FLASH, vaddr as u32, flash_offset, 64, pages as u32, 0);
        Cache_Resume_DCache(0);
        rc
    };
    if rc != 0 {
        return Err(MapError::Rom(rc));
    }
    // SAFETY: the range is now mapped read-only and nothing else maps it.
    Ok(unsafe { core::slice::from_raw_parts(vaddr as *const u8, len) })
}
