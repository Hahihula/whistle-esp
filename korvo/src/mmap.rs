//! Map the model's flash partition into the ESP32-S31 flash window.
//!
//! The S31, like the P4, has two independent MMUs: one for flash (registers
//! in SPI_MEM_C, virtual window 0x4000_0000, 1024 entries of 64 KB) and one
//! for PSRAM (SPI_MEM_S, 0x5000_0000). So unlike the S3 the model doesn't have
//! to share address space with PSRAM: map it right after the app's own
//! flash pages. An entry is `paddr >> 16 | VALID` (ESP-IDF `mmu_ll.h`,
//! `ext_mem_defs.h`); the bootloader keeps the last entry for itself.

use core::ptr::{read_volatile, write_volatile};

/// Flash offset of the `model` partition (partitions.csv).
pub const MODEL_FLASH_OFFSET: u32 = 0x21_0000;
/// Size of the `model` partition.
pub const MODEL_PARTITION_SIZE: usize = 0xDF_0000;

const FLASH_VADDR: usize = 0x4000_0000;
const PAGE: usize = 0x1_0000;
const MMU_ENTRIES: usize = 1024;
const SPI_MEM_C_MMU_ITEM_CONTENT: usize = 0x2050_037C;
const SPI_MEM_C_MMU_ITEM_INDEX: usize = 0x2050_0380;
const SOC_MMU_FLASH_VALID: u32 = 1 << 12;
const CACHE_MAP_L1_DCACHE: u32 = 1 << 4;

unsafe extern "C" {
    fn Cache_Invalidate_Addr(map: u32, addr: u32, size: u32);
}

#[derive(Debug)]
#[allow(dead_code, reason = "logged through Debug")]
pub enum MapError {
    NoEntries { first_free: usize, pages: usize },
}

unsafe fn read_entry(i: usize) -> u32 {
    unsafe {
        write_volatile(SPI_MEM_C_MMU_ITEM_INDEX as *mut u32, i as u32);
        read_volatile(SPI_MEM_C_MMU_ITEM_CONTENT as *const u32)
    }
}

unsafe fn write_entry(i: usize, v: u32) {
    unsafe {
        write_volatile(SPI_MEM_C_MMU_ITEM_INDEX as *mut u32, i as u32);
        write_volatile(SPI_MEM_C_MMU_ITEM_CONTENT as *mut u32, v);
    }
}

/// Maps `len` bytes of flash from `flash_offset` after the highest flash
/// MMU entry in use, and returns the mapped bytes.
///
/// # Safety
/// Call once, before anything else changes the flash MMU.
pub unsafe fn map_flash(flash_offset: u32, len: usize) -> Result<(&'static [u8], usize), MapError> {
    unsafe {
        let mut first_free = 0;
        for i in (0..MMU_ENTRIES - 1).rev() {
            if read_entry(i) & SOC_MMU_FLASH_VALID != 0 {
                first_free = i + 1;
                break;
            }
        }
        let pages = len.div_ceil(PAGE);
        if first_free + pages >= MMU_ENTRIES - 1 {
            return Err(MapError::NoEntries { first_free, pages });
        }
        let page0 = flash_offset as usize / PAGE;
        for p in 0..pages {
            write_entry(first_free + p, (page0 + p) as u32 | SOC_MMU_FLASH_VALID);
        }
        let vaddr = FLASH_VADDR + first_free * PAGE;
        Cache_Invalidate_Addr(CACHE_MAP_L1_DCACHE, vaddr as u32, (pages * PAGE) as u32);
        Ok((
            core::slice::from_raw_parts(vaddr as *const u8, len),
            first_free,
        ))
    }
}
