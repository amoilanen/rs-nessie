//! Mapper 3 (CNROM) — 16 KB or 32 KB fixed PRG-ROM, switchable 8 KB CHR-ROM.
//!
//! The only register is a write to `$8000-$FFFF` that selects the 8 KB CHR
//! bank exposed at `$0000-$1FFF`. PRG behaves exactly like NROM.

use super::{Mapper, Mirroring};

/// CNROM mapper state.
pub struct Mapper003 {
    prg_rom: Vec<u8>,
    chr_rom: Vec<u8>,
    chr_bank_count: usize,
    chr_bank: usize,
    mirroring: Mirroring,
}

impl Mapper003 {
    /// Construct a new CNROM mapper.
    ///
    /// `prg_rom` must be 16 KB or 32 KB. `chr_rom` must be a non-zero multiple
    /// of 8 KB; CNROM cartridges always ship CHR-ROM, not CHR-RAM.
    pub fn new(prg_rom: Vec<u8>, chr_rom: Vec<u8>, mirroring: Mirroring) -> Self {
        debug_assert!(
            prg_rom.len() == 16 * 1024 || prg_rom.len() == 32 * 1024,
            "CNROM PRG-ROM must be 16 KB or 32 KB"
        );
        debug_assert!(
            !chr_rom.is_empty() && chr_rom.len() % (8 * 1024) == 0,
            "CNROM CHR-ROM must be a non-zero multiple of 8 KB"
        );
        let chr_bank_count = chr_rom.len() / (8 * 1024);
        Self {
            prg_rom,
            chr_rom,
            chr_bank_count,
            chr_bank: 0,
            mirroring,
        }
    }

    #[inline]
    fn prg_index(&self, addr: u16) -> usize {
        let offset = (addr - 0x8000) as usize;
        if self.prg_rom.len() == 16 * 1024 {
            offset & 0x3FFF
        } else {
            offset & 0x7FFF
        }
    }
}

impl Mapper for Mapper003 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        if !matches!(addr, 0x8000..=0xFFFF) {
            return 0;
        }
        let idx = self.prg_index(addr);
        self.prg_rom.get(idx).copied().unwrap_or(0)
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if matches!(addr, 0x8000..=0xFFFF) {
            // Only the low bits required to address the available banks; CNROM
            // is famous for treating the high bits as bus conflicts but we
            // emulate the clean case.
            self.chr_bank = (value as usize) & (self.chr_bank_count - 1);
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        let idx = self.chr_bank * 8 * 1024 + (addr & 0x1FFF) as usize;
        self.chr_rom.get(idx).copied().unwrap_or(0)
    }

    fn ppu_write(&mut self, _addr: u16, _value: u8) {
        // CNROM CHR is ROM — writes are dropped.
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Build a CHR-ROM buffer of `bank_count` 8 KB banks where every byte in
    /// bank `b` equals `b`. Lets tests identify the active bank by reading any
    /// CHR address.
    fn chr_with_bank_markers(bank_count: usize) -> Vec<u8> {
        let mut chr = Vec::with_capacity(bank_count * 8 * 1024);
        for bank in 0..bank_count {
            chr.extend(std::iter::repeat(bank as u8).take(8 * 1024));
        }
        chr
    }

    #[test]
    fn chr_bank_switches_on_register_write() {
        let prg = vec![0xAAu8; 16 * 1024];
        let chr = chr_with_bank_markers(4);
        let mut m = Mapper003::new(prg, chr, Mirroring::Horizontal);
        assert_eq!(m.ppu_read(0x0000), 0);
        m.cpu_write(0x8000, 2);
        assert_eq!(m.ppu_read(0x0000), 2);
        assert_eq!(m.ppu_read(0x1FFF), 2);
        m.cpu_write(0xFFFF, 1);
        assert_eq!(m.ppu_read(0x0000), 1);
    }

    #[test]
    fn prg_is_fixed_and_mirrors_for_16k() {
        // 16 KB PRG should mirror into the upper window like NROM.
        let mut prg = vec![0u8; 16 * 1024];
        prg[0] = 0x11;
        prg[0x3FFF] = 0x22;
        let chr = chr_with_bank_markers(1);
        let mut m = Mapper003::new(prg, chr, Mirroring::Horizontal);
        assert_eq!(m.cpu_read(0x8000), 0x11);
        assert_eq!(m.cpu_read(0xC000), 0x11);
        assert_eq!(m.cpu_read(0xBFFF), 0x22);
        assert_eq!(m.cpu_read(0xFFFF), 0x22);
        // Switching the CHR bank must not affect PRG.
        m.cpu_write(0x8000, 0);
        assert_eq!(m.cpu_read(0x8000), 0x11);
    }

    #[test]
    fn chr_writes_are_ignored() {
        let prg = vec![0u8; 16 * 1024];
        let chr = chr_with_bank_markers(2);
        let mut m = Mapper003::new(prg, chr, Mirroring::Horizontal);
        m.ppu_write(0x0000, 0x55);
        assert_eq!(m.ppu_read(0x0000), 0); // bank 0 was filled with 0
    }

    #[test]
    fn chr_bank_select_masked() {
        let prg = vec![0u8; 16 * 1024];
        let chr = chr_with_bank_markers(2);
        let mut m = Mapper003::new(prg, chr, Mirroring::Horizontal);
        // Only 2 banks → 0xFF wraps to bank 1.
        m.cpu_write(0x8000, 0xFF);
        assert_eq!(m.ppu_read(0x0000), 1);
    }
}
