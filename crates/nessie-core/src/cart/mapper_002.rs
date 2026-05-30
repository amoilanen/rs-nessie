//! Mapper 2 (UxROM) — switchable 16 KB PRG bank at `$8000-$BFFF` with the
//! last 16 KB bank fixed at `$C000-$FFFF`. CHR is always 8 KB of CHR-RAM.
//!
//! Writes anywhere in `$8000-$FFFF` set the low PRG bank index. Only as many
//! bits as needed to address every bank are honoured; UxROM carts in the wild
//! use up to 256 KB (4 bits) but masking against `(bank_count - 1)` is the
//! safe general approach.

use super::{Mapper, Mirroring};

/// UxROM mapper state.
pub struct Mapper002 {
    prg_rom: Vec<u8>,
    /// 8 KB CHR-RAM; UxROM never ships CHR-ROM.
    chr_ram: Vec<u8>,
    /// Number of 16 KB PRG banks (always a power of two on real cartridges,
    /// but we mask defensively).
    bank_count: usize,
    /// Currently selected bank for the `$8000-$BFFF` window.
    bank_select: usize,
    /// Fixed last bank index used at `$C000-$FFFF`.
    last_bank: usize,
    mirroring: Mirroring,
}

impl Mapper002 {
    /// Construct a new UxROM mapper from the parsed PRG-ROM buffer.
    ///
    /// `prg_rom` must be a non-empty multiple of 16 KB.
    pub fn new(prg_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        debug_assert!(
            !prg_rom.is_empty() && prg_rom.len() % (16 * 1024) == 0,
            "UxROM PRG-ROM must be a non-zero multiple of 16 KB"
        );
        let bank_count = prg_rom.len() / (16 * 1024);
        let last_bank = bank_count - 1;
        Self {
            prg_rom,
            chr_ram: vec![0u8; 8 * 1024],
            bank_count,
            bank_select: 0,
            last_bank,
            mirroring,
        }
    }

    #[inline]
    fn map_prg(&self, bank: usize, addr_in_bank: usize) -> usize {
        bank * 16 * 1024 + addr_in_bank
    }
}

impl Mapper for Mapper002 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x8000..=0xBFFF => {
                let idx = self.map_prg(self.bank_select, (addr - 0x8000) as usize);
                self.prg_rom.get(idx).copied().unwrap_or(0)
            }
            0xC000..=0xFFFF => {
                let idx = self.map_prg(self.last_bank, (addr - 0xC000) as usize);
                self.prg_rom.get(idx).copied().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if matches!(addr, 0x8000..=0xFFFF) {
            // Mask to the number of available banks (defensive against ROMs
            // that write more bits than they have banks for).
            self.bank_select = (value as usize) & (self.bank_count - 1);
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        let idx = (addr & 0x1FFF) as usize;
        self.chr_ram.get(idx).copied().unwrap_or(0)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        let idx = (addr & 0x1FFF) as usize;
        if let Some(slot) = self.chr_ram.get_mut(idx) {
            *slot = value;
        }
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Build a 64 KB PRG buffer (4 banks) where every byte equals the bank
    /// index. Lets us check which bank a CPU read landed on by inspecting the
    /// returned value alone.
    fn prg_with_bank_markers(bank_count: usize) -> Vec<u8> {
        let mut prg = Vec::with_capacity(bank_count * 16 * 1024);
        for bank in 0..bank_count {
            prg.extend(std::iter::repeat(bank as u8).take(16 * 1024));
        }
        prg
    }

    #[test]
    fn writes_select_low_bank() {
        let mut m = Mapper002::new(prg_with_bank_markers(4), Mirroring::Horizontal);
        // Default bank is 0 → low window reads 0.
        assert_eq!(m.cpu_read(0x8000), 0);
        m.cpu_write(0x8000, 2);
        assert_eq!(m.cpu_read(0x8000), 2);
        assert_eq!(m.cpu_read(0xBFFF), 2);
        m.cpu_write(0xFFFF, 1);
        assert_eq!(m.cpu_read(0x8000), 1);
    }

    #[test]
    fn high_window_always_reads_last_bank() {
        let mut m = Mapper002::new(prg_with_bank_markers(4), Mirroring::Vertical);
        assert_eq!(m.cpu_read(0xC000), 3);
        assert_eq!(m.cpu_read(0xFFFF), 3);
        // Switching the low bank must not change the high window.
        m.cpu_write(0x8000, 2);
        assert_eq!(m.cpu_read(0xC000), 3);
        assert_eq!(m.cpu_read(0xFFFF), 3);
    }

    #[test]
    fn chr_ram_round_trips() {
        let mut m = Mapper002::new(prg_with_bank_markers(2), Mirroring::Horizontal);
        m.ppu_write(0x0042, 0xAB);
        assert_eq!(m.ppu_read(0x0042), 0xAB);
        m.ppu_write(0x1FFF, 0xCD);
        assert_eq!(m.ppu_read(0x1FFF), 0xCD);
    }

    #[test]
    fn bank_select_masked_to_available_banks() {
        // Only 2 banks; writing 3 should wrap to bank 1, not crash.
        let mut m = Mapper002::new(prg_with_bank_markers(2), Mirroring::Horizontal);
        m.cpu_write(0x8000, 0xFF);
        assert_eq!(m.cpu_read(0x8000), 1);
    }

    #[test]
    fn writes_below_8000_are_ignored() {
        let mut m = Mapper002::new(prg_with_bank_markers(4), Mirroring::Horizontal);
        m.cpu_write(0x6000, 2);
        // Bank should remain 0 — write was outside the register window.
        assert_eq!(m.cpu_read(0x8000), 0);
    }
}
