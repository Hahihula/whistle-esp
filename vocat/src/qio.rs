//! Switch the flash to QIO at runtime.
//!
//! espflash writes its `--flash-mode` into the bootloader header too, and the
//! S3 ROM cannot boot a QIO bootloader (`ets_loader.c 78`). ESP-IDF keeps the
//! bootloader header at DIO and lets its bootloader switch; espflash's
//! prebuilt bootloader stays in DIO. So the app does what ESP-IDF's
//! `bootloader_enable_qio_mode()` does:
//!
//! 1. read the JEDEC ID and pick the chip's quad-enable (QE) bit,
//! 2. set QE in the status register if it is clear (WP#/HOLD# become IO2/IO3),
//! 3. route the WP#/HOLD# pads (GPIO28/GPIO27) to the flash controller,
//! 4. switch SPI0's cache reads to QIO: line mode, opcode 0xEB and dummy
//!    cycles. (The ROM's `esp_rom_spiflash_config_readmode` is not enough
//!    here: it leaves the cache sending the DIO opcode 0xBB and the app hangs.)
//!
//! Flash commands go through SPI1's user-command registers, like
//! `bootloader_execute_flash_command`. Everything between suspending and
//! resuming the caches runs from IRAM and touches no flash-mapped data.
//!
//! Call it before PSRAM is in use (invalidating the data cache would drop
//! dirty PSRAM lines) and before the second core starts.

use core::ptr::{read_volatile, write_volatile};

const SPI1: usize = 0x6000_2000;
const SPI0: usize = 0x6000_3000;
const CMD: usize = 0x00;
const CTRL: usize = 0x08;
const USER: usize = 0x18;
const USER1: usize = 0x1C;
const USER2: usize = 0x20;
const MOSI_DLEN: usize = 0x24;
const MISO_DLEN: usize = 0x28;
const W0: usize = 0x58;

const CMD_USR: u32 = 1 << 18;
const CTRL_WP: u32 = 1 << 21;
const USER_MOSI: u32 = 1 << 27;
const USER_MISO: u32 = 1 << 28;
const USER_DUMMY: u32 = 1 << 29;
const USER_ADDR: u32 = 1 << 30;
const USER_COMMAND: u32 = 1 << 31;

const RDID: u8 = 0x9F;
const RDSR: u8 = 0x05;
const RDSR2: u8 = 0x35;
const WREN: u8 = 0x06;
const WRSR: u8 = 0x01;
const WRSR2: u8 = 0x31;

const CTRL_FREAD_DUAL: u32 = 1 << 14;
const CTRL_FREAD_QUAD: u32 = 1 << 20;
const CTRL_FREAD_DIO: u32 = 1 << 23;
const CTRL_FREAD_QIO: u32 = 1 << 24;
const USER1_DUMMY_MASK: u32 = 0x3F;
/// Quad I/O fast read: 24-bit address and 8 mode bits on four lines, then
/// 4 dummy clocks. The mode bits (2 clocks, driven high: no continuous
/// read) count as dummy, so 6 clocks, register value 5 (ESP-IDF:
/// SPI0_R_QIO_DUMMY_CYCLELEN).
const SPI0_R_QIO_DUMMY_CYCLELEN: u32 = 5;
const CMD_FASTRD_QIO: u32 = 0xEB;

unsafe extern "C" {
    fn rom_Cache_Suspend_ICache() -> u32;
    fn Cache_Resume_ICache(autoload: u32);
    fn Cache_Suspend_DCache() -> u32;
    fn Cache_Resume_DCache(autoload: u32);
    fn Cache_Invalidate_ICache_All();
    fn esp_rom_spiflash_select_qio_pins(wp_gpio_num: u8, spiconfig: u32);
    fn Cache_Invalidate_DCache_All();
}

#[derive(Debug)]
#[allow(dead_code, reason = "logged through Debug")]
pub struct Report {
    pub jedec: u32,
    pub status_before: u32,
    pub status_after: u32,
    pub qe_bit: u32,
    pub spi0_ctrl: (u32, u32),
    pub spi0_user1: (u32, u32),
    pub spi0_user2: (u32, u32),
    /// IO_MUX of GPIO27 (HOLD#) and GPIO28 (WP#), before and after.
    pub pads: ((u32, u32), (u32, u32)),
}

#[derive(Debug)]
#[allow(dead_code, reason = "logged through Debug")]
pub enum Error {
    /// The QE bit did not stick after writing the status register.
    QeNotSet(Report),
    /// Data read back differently in QIO; switched back to DIO.
    Mismatch(Report),
}

#[inline(always)]
unsafe fn rd(a: usize) -> u32 {
    unsafe { read_volatile(a as *const u32) }
}

#[inline(always)]
unsafe fn wr(a: usize, v: u32) {
    unsafe { write_volatile(a as *mut u32, v) }
}

/// One flash command on SPI1: 8-bit opcode, up to 32 bits out, up to 32 in.
#[inline(always)]
unsafe fn flash_cmd(cmd: u8, mosi: u32, mosi_bits: u32, miso_bits: u32) -> u32 {
    unsafe {
        let (ctrl, user, user2) = (rd(SPI1 + CTRL), rd(SPI1 + USER), rd(SPI1 + USER2));
        // WP high while idle, single-line command (clears the read-mode bits).
        wr(SPI1 + CTRL, CTRL_WP);
        let mut u = user & !(USER_MOSI | USER_MISO | USER_DUMMY | USER_ADDR | USER_COMMAND);
        u |= USER_COMMAND;
        if miso_bits > 0 {
            u |= USER_MISO;
        }
        if mosi_bits > 0 {
            u |= USER_MOSI;
        }
        wr(SPI1 + USER, u);
        wr(SPI1 + USER2, (7 << 28) | cmd as u32);
        wr(SPI1 + MISO_DLEN, miso_bits.saturating_sub(1));
        wr(SPI1 + MOSI_DLEN, mosi_bits.saturating_sub(1));
        wr(SPI1 + W0, mosi);
        wr(SPI1 + CMD, CMD_USR);
        while rd(SPI1 + CMD) & CMD_USR != 0 {}
        wr(SPI1 + CTRL, ctrl);
        wr(SPI1 + USER, user);
        wr(SPI1 + USER2, user2);
        let r = rd(SPI1 + W0);
        if miso_bits >= 32 { r } else { r & ((1u32 << miso_bits) - 1) }
    }
}

#[inline(always)]
unsafe fn wait_idle() {
    unsafe { while flash_cmd(RDSR, 0, 0, 8) & 1 != 0 {} }
}

// Quad-enable method, from ESP-IDF's `bootloader_flash_qio_*` chip table.
// Plain integers and `if`s rather than an enum and `match`: a lookup table
// would land in flash rodata, which is unreadable while the caches are off.
/// ISSI, MXIC: QE is bit 6 of SR1 (RDSR / WRSR, 8 bit).
const QE_SR1_BIT6: u32 = 0;
/// GigaDevice 1.8 V parts: QE is bit 1 of SR2 (RDSR2 / WRSR2).
const QE_SR2_BIT1: u32 = 1;
/// Everyone else: QE is bit 9 of SR1 | SR2 << 8 (RDSR + RDSR2 / 16-bit WRSR).
const QE_SR16_BIT9: u32 = 2;

#[inline(always)]
unsafe fn read_status(m: u32) -> u32 {
    unsafe {
        if m == QE_SR1_BIT6 {
            flash_cmd(RDSR, 0, 0, 8)
        } else if m == QE_SR2_BIT1 {
            flash_cmd(RDSR2, 0, 0, 8)
        } else {
            flash_cmd(RDSR, 0, 0, 8) | flash_cmd(RDSR2, 0, 0, 8) << 8
        }
    }
}

#[inline(always)]
unsafe fn write_status(m: u32, v: u32) {
    unsafe {
        if m == QE_SR1_BIT6 {
            flash_cmd(WRSR, v, 8, 0);
        } else if m == QE_SR2_BIT1 {
            flash_cmd(WRSR2, v, 8, 0);
        } else {
            flash_cmd(WRSR, v, 16, 0);
        }
    }
}

/// The switch itself, with the caches suspended. IRAM only.
#[esp_hal::ram]
unsafe fn switch(to_qio: bool, saved: (u32, u32, u32, u32, u32)) -> Result<Report, Report> {
    unsafe {
        let ia = rom_Cache_Suspend_ICache();
        let da = Cache_Suspend_DCache();
        let ctrl0 = rd(SPI0 + CTRL);
        let user1_0 = rd(SPI0 + USER1);
        let user2_0 = rd(SPI0 + USER2);
        let pads0 = (rd(IO_MUX_GPIO27), rd(IO_MUX_GPIO28));

        wait_idle();
        let id = flash_cmd(RDID, 0, 0, 24);
        let mfg = id & 0xFF;
        let dev = ((id >> 8) & 0xFF) << 8 | (id >> 16) & 0xFF;
        let (m, bit) = if (mfg == 0x9D && dev & 0xCF00 == 0x4000) || (mfg == 0xC2 && dev & 0xFF00 == 0x2000) {
            (QE_SR1_BIT6, 6)
        } else if mfg == 0xC8 && dev & 0xFF00 == 0x6000 {
            (QE_SR2_BIT1, 1)
        } else {
            (QE_SR16_BIT9, 9)
        };
        let before = read_status(m);
        let mut after = before;
        let mut ok = true;
        if to_qio && before & (1 << bit) == 0 {
            flash_cmd(WREN, 0, 0, 0);
            write_status(m, before | (1 << bit));
            wait_idle();
            after = read_status(m);
            ok = after & (1 << bit) != 0;
        }
        // SPI0 (cache) read setup. The bootloader leaves it in DIO with the
        // opcode taken from USER2 (CACHE_FCTRL.cache_flash_usr_cmd), so the
        // ROM's config_readmode alone is not enough: it changes the line mode
        // but the cache keeps sending 0xBB. Set it like ESP-IDF's
        // spimem_flash_ll_set_read_mode plus the QIO dummy/opcode constants.
        if ok {
            if to_qio {
                // WP#/HOLD# become IO2/IO3: route GPIO28/GPIO27 to the flash
                // controller. In DIO the boot path leaves them as GPIOs, and
                // QIO reads then see IO2/IO3 stuck high.
                esp_rom_spiflash_select_qio_pins(SPI_WP_GPIO, 0);
                wr(SPI0 + CTRL, (ctrl0 & !(CTRL_FREAD_DIO | CTRL_FREAD_DUAL | CTRL_FREAD_QUAD)) | CTRL_FREAD_QIO);
                wr(SPI0 + USER1, (user1_0 & !USER1_DUMMY_MASK) | SPI0_R_QIO_DUMMY_CYCLELEN);
                wr(SPI0 + USER2, (user2_0 & !0xFFFF) | CMD_FASTRD_QIO);
            } else {
                wr(SPI0 + CTRL, saved.0);
                wr(SPI0 + USER1, saved.1);
                wr(SPI0 + USER2, saved.2);
                wr(IO_MUX_GPIO27, saved.3);
                wr(IO_MUX_GPIO28, saved.4);
            }
        }
        Cache_Invalidate_ICache_All();
        Cache_Invalidate_DCache_All();
        let report = Report {
            jedec: id,
            status_before: before,
            status_after: after,
            qe_bit: bit,
            spi0_ctrl: (ctrl0, rd(SPI0 + CTRL)),
            spi0_user1: (user1_0, rd(SPI0 + USER1)),
            spi0_user2: (user2_0, rd(SPI0 + USER2)),
            pads: (pads0, (rd(IO_MUX_GPIO27), rd(IO_MUX_GPIO28))),
        };
        Cache_Resume_DCache(da);
        Cache_Resume_ICache(ia);
        if ok { Ok(report) } else { Err(report) }
    }
}

const CLOCK: usize = 0x14;
const CACHE_FCTRL: usize = 0x3C;

/// Flash identity and the memory controller's current read setup.
#[derive(Debug)]
#[allow(dead_code, reason = "logged through Debug")]
pub struct Regs {
    pub jedec: u32,
    pub sr1: u32,
    pub sr2: u32,
    pub spi0_ctrl: u32,
    pub spi0_clock: u32,
    pub spi0_user: u32,
    pub spi0_user1: u32,
    pub spi0_user2: u32,
    pub spi0_cache_fctrl: u32,
    pub spi1_ctrl: u32,
    pub spi1_user1: u32,
}

#[esp_hal::ram]
unsafe fn read_regs() -> Regs {
    unsafe {
        let ia = rom_Cache_Suspend_ICache();
        let da = Cache_Suspend_DCache();
        wait_idle();
        let r = Regs {
            jedec: flash_cmd(RDID, 0, 0, 24),
            sr1: flash_cmd(RDSR, 0, 0, 8),
            sr2: flash_cmd(RDSR2, 0, 0, 8),
            spi0_ctrl: rd(SPI0 + CTRL),
            spi0_clock: rd(SPI0 + CLOCK),
            spi0_user: rd(SPI0 + USER),
            spi0_user1: rd(SPI0 + USER1),
            spi0_user2: rd(SPI0 + USER2),
            spi0_cache_fctrl: rd(SPI0 + CACHE_FCTRL),
            spi1_ctrl: rd(SPI1 + CTRL),
            spi1_user1: rd(SPI1 + USER1),
        };
        Cache_Resume_DCache(da);
        Cache_Resume_ICache(ia);
        r
    }
}

/// Read-only: what flash this is and how SPI0 currently reads it.
#[allow(dead_code)]
pub fn diagnose() -> Regs {
    critical_section::with(|_| unsafe { read_regs() })
}

const ADDR: usize = 0x04;

/// One QIO fast read (0xEB) of 32 bits at `addr` through SPI1 user registers,
/// with `dummy` SPI clocks after the address (mode bits included).
#[inline(always)]
unsafe fn spi1_qio_read(addr: u32, dummy: u32, spi0_ctrl: u32) -> u32 {
    unsafe {
        let (ctrl, user, user1, user2) = (rd(SPI1 + CTRL), rd(SPI1 + USER), rd(SPI1 + USER1), rd(SPI1 + USER2));
        let c = (spi0_ctrl & !(CTRL_FREAD_DIO | CTRL_FREAD_DUAL | CTRL_FREAD_QUAD)) | CTRL_FREAD_QIO;
        wr(SPI1 + CTRL, c);
        let u = (user & !(USER_MOSI | USER_MISO | USER_DUMMY | USER_ADDR | USER_COMMAND))
            | USER_COMMAND | USER_ADDR | USER_MISO | if dummy > 0 { USER_DUMMY } else { 0 };
        wr(SPI1 + USER, u);
        wr(SPI1 + USER1, (23 << 26) | dummy.saturating_sub(1));
        wr(SPI1 + USER2, (7 << 28) | CMD_FASTRD_QIO);
        wr(SPI1 + ADDR, addr);
        wr(SPI1 + MISO_DLEN, 31);
        wr(SPI1 + CMD, CMD_USR);
        while rd(SPI1 + CMD) & CMD_USR != 0 {}
        let r = rd(SPI1 + W0);
        wr(SPI1 + CTRL, ctrl);
        wr(SPI1 + USER, user);
        wr(SPI1 + USER1, user1);
        wr(SPI1 + USER2, user2);
        r
    }
}

/// IO_MUX registers of the flash's HOLD# (GPIO27) and WP# (GPIO28) pads.
const IO_MUX_GPIO27: usize = 0x6000_9000 + 4 + 4 * 27;
const IO_MUX_GPIO28: usize = 0x6000_9000 + 4 + 4 * 28;
const SPI_WP_GPIO: u8 = 28;

#[allow(dead_code)]
pub fn pad_regs() -> (u32, u32) {
    unsafe { (rd(IO_MUX_GPIO27), rd(IO_MUX_GPIO28)) }
}

#[esp_hal::ram]
unsafe fn sweep_inner(addr: u32, out: &mut [u32; 8], route_pins: bool) {
    unsafe {
        let ia = rom_Cache_Suspend_ICache();
        let da = Cache_Suspend_DCache();
        if route_pins {
            esp_rom_spiflash_select_qio_pins(SPI_WP_GPIO, 0);
        }
        wait_idle();
        let c0 = rd(SPI0 + CTRL);
        let mut i = 0;
        while i < 8 {
            out[i] = spi1_qio_read(addr, i as u32 + 1, c0);
            i += 1;
        }
        Cache_Resume_DCache(da);
        Cache_Resume_ICache(ia);
    }
}

/// Diagnostic, unused in normal runs. It found the two problems above: with
/// the pads unrouted every nibble read back as 0b11xx (IO2/IO3 high), and
/// once routed, 6 dummy clocks gave the right data.
///
/// Safe experiment: QIO reads through SPI1 with 1..=8 dummy clocks while the
/// cache stays in DIO. Entry i is the word read with i + 1 dummy clocks.
#[allow(dead_code)]
pub fn sweep(addr: u32, route_pins: bool) -> [u32; 8] {
    let mut out = [0u32; 8];
    critical_section::with(|_| unsafe { sweep_inner(addr, &mut out, route_pins) });
    out
}

fn checksum(b: &[u8]) -> u32 {
    b.iter().fold(0x811C_9DC5u32, |h, &x| (h ^ x as u32).wrapping_mul(0x0100_0193))
}

/// Switch to QIO and check that `probe` (flash-mapped data) reads back the
/// same. On a mismatch the flash goes back to DIO.
pub fn enable(probe: &[u8]) -> Result<Report, Error> {
    let want = checksum(probe);
    let r = critical_section::with(|_| unsafe { switch(true, (0, 0, 0, 0, 0)) }).map_err(Error::QeNotSet)?;
    // The switch invalidated the caches, so this reads flash in QIO.
    if checksum(probe) != want {
        let saved = (r.spi0_ctrl.0, r.spi0_user1.0, r.spi0_user2.0, r.pads.0.0, r.pads.0.1);
        let _ = critical_section::with(|_| unsafe { switch(false, saved) });
        return Err(Error::Mismatch(r));
    }
    Ok(r)
}
