//! Mapper 1 (MMC1, SxROM) — serial-shift-register bank switching.
//!
//! MMC1 implements bank switching through a 5-bit serial shift register loaded
//! from bit 0 of successive CPU writes to `$8000-$FFFF`. The fifth write
//! commits the shifted value to one of four internal registers determined by
//! bits 13-14 of the destination address:
//!
//! | bits 13-14 | range          | register |
//! |------------|----------------|----------|
//! | 00         | `$8000-$9FFF`  | Control  |
//! | 01         | `$A000-$BFFF`  | CHR bank 0 |
//! | 10         | `$C000-$DFFF`  | CHR bank 1 |
//! | 11         | `$E000-$FFFF`  | PRG bank |
//!
//! A write with bit 7 set resets the shift register and ORs `$0C` into the
//! Control register (selecting "fixed last bank at $C000" mode).

use super::{Mapper, Mirroring};

const PRG_BANK_SIZE: usize = 16 * 1024;
const CHR_BANK_4K: usize = 4 * 1024;
const PRG_RAM_SIZE: usize = 8 * 1024;

/// MMC1 mapper state.
pub struct Mapper001 {
    prg_rom: Vec<u8>,
    /// Either 8 KB of CHR-RAM or a multiple of 8 KB of CHR-ROM.
    chr: Vec<u8>,
    use_chr_ram: bool,
    has_battery: bool,
    prg_ram: Vec<u8>,

    /// 5-bit serial shift register; bit 4 set means "ready to commit".
    shift: u8,
    /// Number of bits already shifted in (0..=4).
    shift_count: u8,

    /// Control register ($8000-$9FFF).
    ctrl: u8,
    /// CHR bank 0 register ($A000-$BFFF).
    chr0: u8,
    /// CHR bank 1 register ($C000-$DFFF).
    chr1: u8,
    /// PRG bank register ($E000-$FFFF): low 4 bits select bank, bit 4 disables
    /// PRG-RAM when set.
    prg: u8,

    prg_bank_count: usize,
}

impl Mapper001 {
    /// Construct a new MMC1 mapper from the parsed PRG/CHR buffers.
    pub fn new(prg_rom: Vec<u8>, chr: Vec<u8>, use_chr_ram: bool, has_battery: bool) -> Self {
        debug_assert!(
            !prg_rom.is_empty() && prg_rom.len() % PRG_BANK_SIZE == 0,
            "MMC1 PRG-ROM must be a non-zero multiple of 16 KB"
        );
        let prg_bank_count = prg_rom.len() / PRG_BANK_SIZE;
        Self {
            prg_rom,
            chr,
            use_chr_ram,
            has_battery,
            prg_ram: vec![0u8; PRG_RAM_SIZE],
            shift: 0,
            shift_count: 0,
            // Power-on default per the MMC1 spec: PRG mode 3 (fixed last bank
            // at $C000), CHR mode 0, mirroring = one-screen lower.
            ctrl: 0x0C,
            chr0: 0,
            chr1: 0,
            prg: 0,
            prg_bank_count,
        }
    }

    /// Resolve the active mirroring mode from the Control register.
    fn current_mirroring(&self) -> Mirroring {
        match self.ctrl & 0x03 {
            0 => Mirroring::OneScreenLower,
            1 => Mirroring::OneScreenUpper,
            2 => Mirroring::Vertical,
            // SAFETY: the mask `& 0x03` confines the value to `0..=3`.
            _ => Mirroring::Horizontal,
        }
    }

    /// PRG bank-mode field (Control bits 2-3).
    #[inline]
    fn prg_mode(&self) -> u8 {
        (self.ctrl >> 2) & 0x03
    }

    /// CHR bank-mode field (Control bit 4). `true` → two 4 KB banks; `false`
    /// → single 8 KB bank (chr0 with low bit ignored).
    #[inline]
    fn chr_mode_4k(&self) -> bool {
        self.ctrl & 0x10 != 0
    }

    /// Translate a CPU PRG-ROM address into a flat byte index into `prg_rom`.
    fn prg_byte_index(&self, addr: u16) -> usize {
        let last_bank = self.prg_bank_count - 1;
        // `prg` low 4 bits select a 16 KB bank; in practice MMC1 carts have
        // up to 16 banks (256 KB).
        let bank_sel = (self.prg & 0x0F) as usize & (self.prg_bank_count - 1);
        let (low_bank, high_bank) = match self.prg_mode() {
            0 | 1 => {
                // 32 KB switch — ignore low bit of bank select.
                let base = bank_sel & !1;
                (base, base + 1)
            }
            2 => {
                // Fixed first bank at $8000, switchable at $C000.
                (0, bank_sel)
            }
            // mode 3
            _ => {
                // Switchable at $8000, fixed last bank at $C000.
                (bank_sel, last_bank)
            }
        };
        let (bank, addr_in_bank) = if addr < 0xC000 {
            (low_bank, (addr - 0x8000) as usize)
        } else {
            (high_bank, (addr - 0xC000) as usize)
        };
        bank * PRG_BANK_SIZE + addr_in_bank
    }

    /// Translate a PPU CHR address (`$0000-$1FFF`) into a flat byte index into
    /// `chr`.
    fn chr_byte_index(&self, addr: u16) -> usize {
        let addr = (addr & 0x1FFF) as usize;
        if self.use_chr_ram {
            // CHR-RAM is always a flat 8 KB region in this implementation.
            return addr;
        }
        let total_4k_banks = self.chr.len() / CHR_BANK_4K;
        // Defensive mask: bank index wraps to the available bank count.
        let bank_mask = if total_4k_banks == 0 {
            0
        } else {
            total_4k_banks - 1
        };
        if self.chr_mode_4k() {
            // Two independent 4 KB banks.
            let (bank, off) = if addr < CHR_BANK_4K {
                ((self.chr0 as usize) & bank_mask, addr)
            } else {
                ((self.chr1 as usize) & bank_mask, addr - CHR_BANK_4K)
            };
            bank * CHR_BANK_4K + off
        } else {
            // Single 8 KB bank — chr0 with low bit cleared.
            let base = (self.chr0 as usize) & !1 & bank_mask;
            base * CHR_BANK_4K + addr
        }
    }

    /// Apply the shift-register reset behaviour triggered by any CPU write with
    /// bit 7 set to `$8000-$FFFF`.
    fn reset_shift(&mut self) {
        self.shift = 0;
        self.shift_count = 0;
        // Per the MMC1 spec the reset also ORs `$0C` into the control register
        // (locks PRG mode 3 — fixed last bank at $C000).
        self.ctrl |= 0x0C;
    }

    /// Commit the current shift register value to the target register selected
    /// by bits 13-14 of `addr`.
    fn commit(&mut self, addr: u16, value: u8) {
        let value = value & 0x1F;
        match (addr >> 13) & 0x03 {
            0 => self.ctrl = value,
            1 => self.chr0 = value,
            2 => self.chr1 = value,
            _ => self.prg = value,
        }
    }

    /// `true` when the PRG-RAM at `$6000-$7FFF` is enabled (PRG register bit 4
    /// clear).
    #[inline]
    fn prg_ram_enabled(&self) -> bool {
        self.prg & 0x10 == 0
    }
}

impl Mapper for Mapper001 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => {
                if !self.prg_ram_enabled() {
                    return 0;
                }
                self.prg_ram
                    .get((addr - 0x6000) as usize)
                    .copied()
                    .unwrap_or(0)
            }
            0x8000..=0xFFFF => {
                let idx = self.prg_byte_index(addr);
                self.prg_rom.get(idx).copied().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if !self.prg_ram_enabled() {
                    return;
                }
                if let Some(slot) = self.prg_ram.get_mut((addr - 0x6000) as usize) {
                    *slot = value;
                }
            }
            0x8000..=0xFFFF => {
                if value & 0x80 != 0 {
                    self.reset_shift();
                    return;
                }
                // Shift bit 0 of the value into bit 4 of the shift register; the
                // value clocks in LSB-first.
                self.shift = (self.shift >> 1) | ((value & 1) << 4);
                self.shift_count += 1;
                if self.shift_count == 5 {
                    let to_commit = self.shift;
                    self.commit(addr, to_commit);
                    self.shift = 0;
                    self.shift_count = 0;
                }
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        let idx = self.chr_byte_index(addr);
        self.chr.get(idx).copied().unwrap_or(0)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        if !self.use_chr_ram {
            return;
        }
        let idx = self.chr_byte_index(addr);
        if let Some(slot) = self.chr.get_mut(idx) {
            *slot = value;
        }
    }

    fn mirroring(&self) -> Mirroring {
        self.current_mirroring()
    }

    fn battery_ram(&self) -> Option<&[u8]> {
        if self.has_battery {
            Some(&self.prg_ram)
        } else {
            None
        }
    }

    fn load_battery(&mut self, bytes: &[u8]) {
        if !self.has_battery {
            return;
        }
        let n = bytes.len().min(self.prg_ram.len());
        self.prg_ram[..n].copy_from_slice(&bytes[..n]);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Build a PRG-ROM where each byte equals its containing 16 KB bank
    /// number. Lets tests read a single byte to identify the active bank.
    fn prg_bank_markers(bank_count: usize) -> Vec<u8> {
        let mut prg = Vec::with_capacity(bank_count * PRG_BANK_SIZE);
        for bank in 0..bank_count {
            prg.extend(std::iter::repeat(bank as u8).take(PRG_BANK_SIZE));
        }
        prg
    }

    /// Build a CHR-ROM where each byte equals its containing 4 KB bank number.
    fn chr_bank_markers(bank_4k_count: usize) -> Vec<u8> {
        let mut chr = Vec::with_capacity(bank_4k_count * CHR_BANK_4K);
        for bank in 0..bank_4k_count {
            chr.extend(std::iter::repeat(bank as u8).take(CHR_BANK_4K));
        }
        chr
    }

    /// Helper: write a 5-bit value `v` to MMC1 register at `addr`.
    fn serial_write(m: &mut Mapper001, addr: u16, v: u8) {
        for i in 0..5 {
            m.cpu_write(addr, (v >> i) & 1);
        }
    }

    #[test]
    fn power_on_defaults_to_prg_mode_3() {
        // Default ctrl=0x0C → mode 3 → first window switchable (bank 0),
        // high window fixed at last bank.
        let prg = prg_bank_markers(4);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        assert_eq!(m.cpu_read(0x8000), 0);
        assert_eq!(m.cpu_read(0xC000), 3);
        assert_eq!(m.cpu_read(0xFFFF), 3);
    }

    #[test]
    fn reset_high_bit_resets_shift_and_or_in_0c() {
        let prg = prg_bank_markers(2);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        // Shift in two bits first.
        m.cpu_write(0x8000, 1);
        m.cpu_write(0x8000, 1);
        assert_eq!(m.shift_count, 2);
        // Now a write with bit 7 set must reset.
        m.cpu_write(0x8000, 0x80);
        assert_eq!(m.shift_count, 0);
        assert_eq!(m.shift, 0);
        // Control low 4 bits OR'd with 0x0C — power-on already has 0x0C so
        // this is still 0x0C → PRG mode 3.
        assert_eq!(m.ctrl & 0x0C, 0x0C);
    }

    #[test]
    fn control_register_decoding_sets_mirroring() {
        let prg = prg_bank_markers(2);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        // Write mirroring=vertical (mode 2), prg mode 3 (0b11), chr mode 0 → 0b01110 = 0x0E
        serial_write(&mut m, 0x8000, 0x0E);
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        serial_write(&mut m, 0x8000, 0x0F);
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
        serial_write(&mut m, 0x8000, 0x0C);
        assert_eq!(m.mirroring(), Mirroring::OneScreenLower);
        serial_write(&mut m, 0x8000, 0x0D);
        assert_eq!(m.mirroring(), Mirroring::OneScreenUpper);
    }

    #[test]
    fn prg_mode_3_low_switchable_high_fixed() {
        let prg = prg_bank_markers(4);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        // Default ctrl already mode 3; select bank 2 at $8000.
        serial_write(&mut m, 0xE000, 2);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.cpu_read(0xC000), 3); // last bank fixed
    }

    #[test]
    fn prg_mode_2_low_fixed_high_switchable() {
        let prg = prg_bank_markers(4);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        // Control: mirroring 0 (one-screen-lower), prg mode 2 (0b10 → bits 2-3),
        // chr mode 0 → 0b01000 = 0x08
        serial_write(&mut m, 0x8000, 0x08);
        // PRG bank register → 2.
        serial_write(&mut m, 0xE000, 2);
        assert_eq!(m.cpu_read(0x8000), 0); // first bank fixed
        assert_eq!(m.cpu_read(0xC000), 2); // switchable
    }

    #[test]
    fn prg_mode_0_32k_switch_ignores_low_bit() {
        let prg = prg_bank_markers(4);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        // Control: prg mode 0 (bits 2-3 = 0), chr mode 0 → 0
        serial_write(&mut m, 0x8000, 0x00);
        // PRG bank = 3 (odd) → 32 KB switch ignores low bit → base bank 2.
        serial_write(&mut m, 0xE000, 3);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.cpu_read(0xC000), 3);
    }

    #[test]
    fn chr_mode_4k_maps_two_independent_banks() {
        let prg = prg_bank_markers(2);
        let chr = chr_bank_markers(8); // 8 × 4 KB = 32 KB CHR
        let mut m = Mapper001::new(prg, chr, false, false);
        // Control: mirroring 0, prg mode 3, chr mode 1 (4 KB) → 0b11100 = 0x1C
        serial_write(&mut m, 0x8000, 0x1C);
        // chr0 = 5 → $0000-$0FFF reads bank 5.
        serial_write(&mut m, 0xA000, 5);
        // chr1 = 2 → $1000-$1FFF reads bank 2.
        serial_write(&mut m, 0xC000, 2);
        assert_eq!(m.ppu_read(0x0000), 5);
        assert_eq!(m.ppu_read(0x0FFF), 5);
        assert_eq!(m.ppu_read(0x1000), 2);
        assert_eq!(m.ppu_read(0x1FFF), 2);
    }

    #[test]
    fn chr_mode_8k_ignores_low_bit_of_chr0() {
        let prg = prg_bank_markers(2);
        let chr = chr_bank_markers(8);
        let mut m = Mapper001::new(prg, chr, false, false);
        // Control: prg mode 3, chr mode 0 → 0x0C
        serial_write(&mut m, 0x8000, 0x0C);
        // chr0 = 5 → 8 KB mode masks low bit → base bank 4.
        serial_write(&mut m, 0xA000, 5);
        assert_eq!(m.ppu_read(0x0000), 4);
        assert_eq!(m.ppu_read(0x1000), 5); // second half of 8 KB = bank 5
    }

    #[test]
    fn prg_ram_enable_bit_gates_reads_and_writes() {
        let prg = prg_bank_markers(2);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, true);
        // Enabled by default; round trip works.
        m.cpu_write(0x6000, 0xAB);
        assert_eq!(m.cpu_read(0x6000), 0xAB);
        // PRG register bit 4 set → RAM disabled. Encode bits: 0b10000 = 0x10.
        serial_write(&mut m, 0xE000, 0x10);
        // Disabled writes are dropped; reads return 0.
        m.cpu_write(0x6000, 0xCD);
        assert_eq!(m.cpu_read(0x6000), 0);
        // Re-enable.
        serial_write(&mut m, 0xE000, 0x00);
        // Previous 0xAB still in RAM (disabled writes never landed).
        assert_eq!(m.cpu_read(0x6000), 0xAB);
    }

    #[test]
    fn battery_snapshot_round_trip() {
        let prg = prg_bank_markers(2);
        let mut m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, true);
        m.cpu_write(0x6000, 0x42);
        let snap = m.battery_ram().expect("battery snapshot expected");
        assert_eq!(snap[0], 0x42);
        // Build a fresh mapper and restore.
        let prg2 = prg_bank_markers(2);
        let mut m2 = Mapper001::new(prg2, vec![0u8; 8 * 1024], true, true);
        m2.load_battery(snap);
        assert_eq!(m2.cpu_read(0x6000), 0x42);
    }

    #[test]
    fn no_battery_returns_none_for_snapshot() {
        let prg = prg_bank_markers(2);
        let m = Mapper001::new(prg, vec![0u8; 8 * 1024], true, false);
        assert!(m.battery_ram().is_none());
    }

    #[test]
    fn chr_ram_writes_round_trip() {
        let prg = prg_bank_markers(2);
        let chr = vec![0u8; 8 * 1024];
        let mut m = Mapper001::new(prg, chr, true, false);
        m.ppu_write(0x0123, 0x77);
        assert_eq!(m.ppu_read(0x0123), 0x77);
    }
}
