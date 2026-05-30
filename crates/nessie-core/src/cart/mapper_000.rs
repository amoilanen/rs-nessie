//! Mapper 0 (NROM) — the simplest NES cartridge type.
//!
//! NROM cartridges contain either 16 KB or 32 KB of PRG-ROM and 8 KB of CHR-ROM
//! (or 8 KB of CHR-RAM if the header advertises zero CHR-ROM banks). They have
//! no bank-switching registers; CPU writes to `$8000-$FFFF` are dropped on the
//! floor on real hardware (the ROM chip has no write line wired). Optional
//! 8 KB PRG-RAM may be wired at `$6000-$7FFF`; the NES test-cart family relies
//! on this.

use super::{Mapper, Mirroring};

/// NROM mapper state.
pub struct Mapper000 {
    prg_rom: Vec<u8>,
    /// Backing memory for CHR — either ROM (read-only) or RAM (read/write).
    chr: Vec<u8>,
    /// 8 KB PRG-RAM mapped at `$6000-$7FFF`. Allocated unconditionally; battery
    /// behaviour gates whether it is persisted across sessions.
    prg_ram: Vec<u8>,
    /// `true` when CHR is RAM and `ppu_write` should update the backing memory.
    use_chr_ram: bool,
    /// `true` when the header advertised battery-backed PRG-RAM.
    has_battery: bool,
    mirroring: Mirroring,
}

impl Mapper000 {
    /// Construct a new NROM mapper from the parsed PRG/CHR byte buffers.
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        mirroring: Mirroring,
        use_chr_ram: bool,
        has_battery: bool,
    ) -> Self {
        debug_assert!(
            prg_rom.len() == 16 * 1024 || prg_rom.len() == 32 * 1024,
            "NROM PRG-ROM must be 16 KB or 32 KB"
        );
        Self {
            prg_rom,
            chr,
            prg_ram: vec![0u8; 8 * 1024],
            use_chr_ram,
            has_battery,
            mirroring,
        }
    }

    /// Map a CPU address in `$8000-$FFFF` to a PRG-ROM byte index, accounting
    /// for the 16 KB → mirror-into-`$C000-$FFFF` quirk on small carts.
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

impl Mapper for Mapper000 {
    fn cpu_read(&mut self, addr: u16) -> u8 {
        match addr {
            0x6000..=0x7FFF => {
                let idx = (addr - 0x6000) as usize;
                self.prg_ram.get(idx).copied().unwrap_or(0)
            }
            0x8000..=0xFFFF => {
                let idx = self.prg_index(addr);
                // Index is always in bounds by construction of `prg_index`.
                self.prg_rom.get(idx).copied().unwrap_or(0)
            }
            // Open bus — quiescent NES cartridges return 0 for unmapped reads.
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        if let 0x6000..=0x7FFF = addr {
            let idx = (addr - 0x6000) as usize;
            if let Some(slot) = self.prg_ram.get_mut(idx) {
                *slot = value;
            }
        }
        // Writes to PRG-ROM ($8000-$FFFF) are ignored, matching real hardware.
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        let idx = (addr & 0x1FFF) as usize;
        self.chr.get(idx).copied().unwrap_or(0)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        if !self.use_chr_ram {
            // CHR-ROM is read-only; writes are silently dropped.
            return;
        }
        let idx = (addr & 0x1FFF) as usize;
        if let Some(slot) = self.chr.get_mut(idx) {
            *slot = value;
        }
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
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
