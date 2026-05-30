//! Mapper 4 (MMC3, TxROM) — bank-select with scanline-counter IRQ.
//!
//! MMC3 exposes eight bank registers (R0..R7) selected by a 3-bit index in the
//! "bank select" register at `$8000` (even). Writes to `$8001` (odd) latch the
//! data into the currently selected register. CHR and PRG modes (bits 6 and 7
//! of bank-select) decide which of the 8 KB PRG and 1/2 KB CHR windows the
//! registers map into.
//!
//! The IRQ counter is clocked by rising edges of PPU address line A12. The
//! cleanest place to observe these in this code base is inside [`ppu_read`],
//! so the mapper tracks the previous A12 state itself rather than relying on
//! the PPU to call a dedicated entry point.
//!
//! When the counter is zero (or a reload was requested with `$C001`) the next
//! A12 rising edge reloads from the latch; otherwise it decrements. Hitting
//! zero with IRQs enabled asserts `/IRQ` until acknowledged via `$E000`.

use super::{Mapper, Mirroring};

const PRG_BANK_SIZE_8K: usize = 8 * 1024;
const CHR_BANK_SIZE_1K: usize = 1024;
const PRG_RAM_SIZE: usize = 8 * 1024;

/// MMC3 mapper state.
pub struct Mapper004 {
    prg_rom: Vec<u8>,
    chr: Vec<u8>,
    use_chr_ram: bool,
    has_battery: bool,
    prg_ram: Vec<u8>,

    /// Eight bank registers (R0..R7) selected by `bank_select & 0x07`.
    bank_regs: [u8; 8],
    /// Last write to `$8000` (even): low 3 bits = register index,
    /// bit 6 = PRG mode, bit 7 = CHR mode.
    bank_select: u8,

    mirroring: Mirroring,
    /// PRG-RAM protect ($A001): bit 7 enables, bit 6 write-protects.
    prg_ram_protect: u8,

    /// IRQ counter.
    irq_counter: u8,
    irq_latch: u8,
    irq_reload: bool,
    irq_enabled: bool,
    irq_pending: bool,

    /// Tracks the last observed A12 state for rising-edge detection.
    last_a12: bool,

    prg_bank_count_8k: usize,
    chr_bank_count_1k: usize,
}

impl Mapper004 {
    /// Construct a new MMC3 mapper from the parsed PRG/CHR buffers.
    pub fn new(
        prg_rom: Vec<u8>,
        chr: Vec<u8>,
        mirroring: Mirroring,
        use_chr_ram: bool,
        has_battery: bool,
    ) -> Self {
        debug_assert!(
            !prg_rom.is_empty() && prg_rom.len() % PRG_BANK_SIZE_8K == 0,
            "MMC3 PRG-ROM must be a non-zero multiple of 8 KB"
        );
        let prg_bank_count_8k = prg_rom.len() / PRG_BANK_SIZE_8K;
        let chr_bank_count_1k = if use_chr_ram {
            chr.len() / CHR_BANK_SIZE_1K
        } else {
            (chr.len() / CHR_BANK_SIZE_1K).max(1)
        };
        Self {
            prg_rom,
            chr,
            use_chr_ram,
            has_battery,
            prg_ram: vec![0u8; PRG_RAM_SIZE],
            bank_regs: [0; 8],
            bank_select: 0,
            mirroring,
            prg_ram_protect: 0,
            irq_counter: 0,
            irq_latch: 0,
            irq_reload: false,
            irq_enabled: false,
            irq_pending: false,
            last_a12: false,
            prg_bank_count_8k,
            chr_bank_count_1k,
        }
    }

    #[inline]
    fn prg_mode(&self) -> u8 {
        (self.bank_select >> 6) & 1
    }

    #[inline]
    fn chr_mode(&self) -> u8 {
        (self.bank_select >> 7) & 1
    }

    /// Translate a CPU address in `$8000-$FFFF` into a flat PRG-ROM byte index.
    fn prg_index(&self, addr: u16) -> usize {
        let last = self.prg_bank_count_8k - 1;
        let second_last = last.saturating_sub(1);
        let r6 = (self.bank_regs[6] as usize) & (self.prg_bank_count_8k - 1);
        let r7 = (self.bank_regs[7] as usize) & (self.prg_bank_count_8k - 1);
        let window = (addr - 0x8000) as usize / PRG_BANK_SIZE_8K; // 0..=3
        let bank = match (self.prg_mode(), window) {
            // PRG mode 0: $8000 = R6, $A000 = R7, $C000 = -2, $E000 = -1
            (0, 0) => r6,
            (0, 1) => r7,
            (0, 2) => second_last,
            (0, 3) => last,
            // PRG mode 1: $8000 = -2, $A000 = R7, $C000 = R6, $E000 = -1
            (1, 0) => second_last,
            (1, 1) => r7,
            (1, 2) => r6,
            (1, 3) => last,
            _ => last,
        };
        let off = (addr as usize) - 0x8000 - window * PRG_BANK_SIZE_8K;
        bank * PRG_BANK_SIZE_8K + off
    }

    /// Translate a PPU CHR address (`$0000-$1FFF`) into a flat CHR byte index.
    fn chr_index(&self, addr: u16) -> usize {
        let addr = (addr & 0x1FFF) as usize;
        let bank_mask = if self.chr_bank_count_1k == 0 {
            0
        } else {
            self.chr_bank_count_1k - 1
        };
        // R0/R1 are 2 KB banks (low bit ignored); R2..R5 are 1 KB banks.
        let r0 = (self.bank_regs[0] as usize) & !1 & bank_mask;
        let r1 = (self.bank_regs[1] as usize) & !1 & bank_mask;
        let r2 = (self.bank_regs[2] as usize) & bank_mask;
        let r3 = (self.bank_regs[3] as usize) & bank_mask;
        let r4 = (self.bank_regs[4] as usize) & bank_mask;
        let r5 = (self.bank_regs[5] as usize) & bank_mask;

        let window_1k = addr / CHR_BANK_SIZE_1K; // 0..=7
        let off = addr % CHR_BANK_SIZE_1K;
        // CHR mode 0: R0 → $0000-$07FF (2 KB), R1 → $0800-$0FFF (2 KB),
        //             R2..R5 → 1 KB each at $1000, $1400, $1800, $1C00.
        // CHR mode 1: R2..R5 → 1 KB at $0000..$0FFF, then R0/R1 (2 KB) at
        //             $1000-$1FFF.
        let bank = match (self.chr_mode(), window_1k) {
            (0, 0) => r0,
            (0, 1) => r0 + 1,
            (0, 2) => r1,
            (0, 3) => r1 + 1,
            (0, 4) => r2,
            (0, 5) => r3,
            (0, 6) => r4,
            (0, 7) => r5,
            (1, 0) => r2,
            (1, 1) => r3,
            (1, 2) => r4,
            (1, 3) => r5,
            (1, 4) => r0,
            (1, 5) => r0 + 1,
            (1, 6) => r1,
            (1, 7) => r1 + 1,
            _ => 0,
        };
        bank * CHR_BANK_SIZE_1K + off
    }

    /// Clock the MMC3 IRQ counter on a single A12 rising edge.
    fn clock_irq_counter(&mut self) {
        if self.irq_counter == 0 || self.irq_reload {
            self.irq_counter = self.irq_latch;
            self.irq_reload = false;
        } else {
            self.irq_counter -= 1;
        }
        if self.irq_counter == 0 && self.irq_enabled {
            self.irq_pending = true;
        }
    }

    /// Inspect the address bit 12 of a PPU access and clock the IRQ counter on
    /// a 0→1 transition.
    fn observe_a12(&mut self, addr: u16) {
        let a12 = addr & 0x1000 != 0;
        if a12 && !self.last_a12 {
            self.clock_irq_counter();
        }
        self.last_a12 = a12;
    }

    /// `true` while PRG-RAM is mapped readable (`$A001` bit 7 set).
    #[inline]
    fn prg_ram_enabled(&self) -> bool {
        self.prg_ram_protect & 0x80 != 0
    }

    /// `true` while PRG-RAM writes are blocked (`$A001` bit 6 set).
    #[inline]
    fn prg_ram_write_protected(&self) -> bool {
        self.prg_ram_protect & 0x40 != 0
    }
}

impl Mapper for Mapper004 {
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
                let idx = self.prg_index(addr);
                self.prg_rom.get(idx).copied().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn cpu_write(&mut self, addr: u16, value: u8) {
        match addr {
            0x6000..=0x7FFF => {
                if !self.prg_ram_enabled() || self.prg_ram_write_protected() {
                    return;
                }
                if let Some(slot) = self.prg_ram.get_mut((addr - 0x6000) as usize) {
                    *slot = value;
                }
            }
            0x8000..=0x9FFF => {
                if addr & 1 == 0 {
                    // Bank select.
                    self.bank_select = value;
                } else {
                    // Bank data → R[bank_select & 7].
                    let reg = (self.bank_select & 0x07) as usize;
                    self.bank_regs[reg] = value;
                }
            }
            0xA000..=0xBFFF => {
                if addr & 1 == 0 {
                    // Mirroring (ignored if four-screen). 0 = vertical, 1 = horizontal.
                    if !matches!(self.mirroring, Mirroring::FourScreen) {
                        self.mirroring = if value & 1 == 0 {
                            Mirroring::Vertical
                        } else {
                            Mirroring::Horizontal
                        };
                    }
                } else {
                    self.prg_ram_protect = value;
                }
            }
            0xC000..=0xDFFF => {
                if addr & 1 == 0 {
                    self.irq_latch = value;
                } else {
                    // Force a reload on the next A12 rising edge.
                    self.irq_counter = 0;
                    self.irq_reload = true;
                }
            }
            0xE000..=0xFFFF => {
                if addr & 1 == 0 {
                    // Disable and acknowledge.
                    self.irq_enabled = false;
                    self.irq_pending = false;
                } else {
                    self.irq_enabled = true;
                }
            }
            _ => {}
        }
    }

    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.observe_a12(addr);
        let idx = self.chr_index(addr);
        self.chr.get(idx).copied().unwrap_or(0)
    }

    fn ppu_write(&mut self, addr: u16, value: u8) {
        self.observe_a12(addr);
        if !self.use_chr_ram {
            return;
        }
        let idx = self.chr_index(addr);
        if let Some(slot) = self.chr.get_mut(idx) {
            *slot = value;
        }
    }

    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }

    fn irq_pending(&self) -> bool {
        self.irq_pending
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

    /// Build a PRG-ROM where each byte equals its 8 KB bank index.
    fn prg_8k_markers(bank_count: usize) -> Vec<u8> {
        let mut prg = Vec::with_capacity(bank_count * PRG_BANK_SIZE_8K);
        for bank in 0..bank_count {
            prg.extend(std::iter::repeat(bank as u8).take(PRG_BANK_SIZE_8K));
        }
        prg
    }

    /// Build a CHR-ROM where each byte equals its 1 KB bank index.
    fn chr_1k_markers(bank_count: usize) -> Vec<u8> {
        let mut chr = Vec::with_capacity(bank_count * CHR_BANK_SIZE_1K);
        for bank in 0..bank_count {
            chr.extend(std::iter::repeat(bank as u8).take(CHR_BANK_SIZE_1K));
        }
        chr
    }

    /// Pulse PPU A12 high then low to clock the IRQ counter once.
    fn pulse_a12(m: &mut Mapper004) {
        // Read at $1000 (A12=1) then $0000 (A12=0).
        let _ = m.ppu_read(0x1000);
        let _ = m.ppu_read(0x0000);
    }

    #[test]
    fn prg_mode_0_maps_r6_r7_then_fixed_tail() {
        // 8 banks × 8 KB = 64 KB PRG.
        let prg = prg_8k_markers(8);
        let mut m = Mapper004::new(prg, chr_1k_markers(8), Mirroring::Horizontal, false, false);
        // Bank select with PRG mode 0: bits 6-7 = 0. Select R6 (index 6), assign bank 4.
        m.cpu_write(0x8000, 6);
        m.cpu_write(0x8001, 4);
        // Select R7, assign bank 5.
        m.cpu_write(0x8000, 7);
        m.cpu_write(0x8001, 5);
        // Windows: $8000=R6=4, $A000=R7=5, $C000=second-last=6, $E000=last=7.
        assert_eq!(m.cpu_read(0x8000), 4);
        assert_eq!(m.cpu_read(0xA000), 5);
        assert_eq!(m.cpu_read(0xC000), 6);
        assert_eq!(m.cpu_read(0xE000), 7);
    }

    #[test]
    fn prg_mode_1_swaps_first_and_third_windows() {
        let prg = prg_8k_markers(8);
        let mut m = Mapper004::new(prg, chr_1k_markers(8), Mirroring::Horizontal, false, false);
        // PRG mode 1 → bank-select bit 6 set. Select R6, assign 4. Select R7, assign 5.
        m.cpu_write(0x8000, 0x40 | 6);
        m.cpu_write(0x8001, 4);
        m.cpu_write(0x8000, 0x40 | 7);
        m.cpu_write(0x8001, 5);
        // Now: $8000=second-last=6, $A000=R7=5, $C000=R6=4, $E000=last=7.
        assert_eq!(m.cpu_read(0x8000), 6);
        assert_eq!(m.cpu_read(0xA000), 5);
        assert_eq!(m.cpu_read(0xC000), 4);
        assert_eq!(m.cpu_read(0xE000), 7);
    }

    #[test]
    fn chr_mode_0_maps_2k_then_four_1k_banks() {
        let chr = chr_1k_markers(8);
        let mut m = Mapper004::new(prg_8k_markers(4), chr, Mirroring::Horizontal, false, false);
        // CHR mode 0: bit 7 = 0. Set R0=0 (2 KB low), R1=2 (2 KB), R2..R5=4..7.
        for (reg, val) in [(0u8, 0), (1, 2), (2, 4), (3, 5), (4, 6), (5, 7)] {
            m.cpu_write(0x8000, reg);
            m.cpu_write(0x8001, val);
        }
        assert_eq!(m.ppu_read(0x0000), 0); // R0 low half
        assert_eq!(m.ppu_read(0x0400), 1); // R0 high half (low bit ignored, +1)
        assert_eq!(m.ppu_read(0x0800), 2); // R1
        assert_eq!(m.ppu_read(0x0C00), 3); // R1 + 1
        assert_eq!(m.ppu_read(0x1000), 4); // R2
        assert_eq!(m.ppu_read(0x1400), 5); // R3
        assert_eq!(m.ppu_read(0x1800), 6); // R4
        assert_eq!(m.ppu_read(0x1C00), 7); // R5
    }

    #[test]
    fn chr_mode_1_inverts_layout() {
        let chr = chr_1k_markers(8);
        let mut m = Mapper004::new(prg_8k_markers(4), chr, Mirroring::Horizontal, false, false);
        // CHR mode 1 → bit 7 of bank-select set. Same register assignments.
        for (reg, val) in [(0u8, 0), (1, 2), (2, 4), (3, 5), (4, 6), (5, 7)] {
            m.cpu_write(0x8000, 0x80 | reg);
            m.cpu_write(0x8001, val);
        }
        // CHR mode 1: R2..R5 → $0000-$0FFF (1 KB each), R0/R1 → $1000-$1FFF (2 KB each).
        assert_eq!(m.ppu_read(0x0000), 4); // R2
        assert_eq!(m.ppu_read(0x0400), 5); // R3
        assert_eq!(m.ppu_read(0x0800), 6); // R4
        assert_eq!(m.ppu_read(0x0C00), 7); // R5
        assert_eq!(m.ppu_read(0x1000), 0); // R0 low
        assert_eq!(m.ppu_read(0x1400), 1); // R0 high
        assert_eq!(m.ppu_read(0x1800), 2); // R1 low
        assert_eq!(m.ppu_read(0x1C00), 3); // R1 high
    }

    #[test]
    fn mirroring_register_overrides() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            false,
        );
        m.cpu_write(0xA000, 0); // vertical
        assert_eq!(m.mirroring(), Mirroring::Vertical);
        m.cpu_write(0xA000, 1); // horizontal
        assert_eq!(m.mirroring(), Mirroring::Horizontal);
    }

    #[test]
    fn four_screen_ignores_mirroring_writes() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::FourScreen,
            false,
            false,
        );
        m.cpu_write(0xA000, 0);
        assert_eq!(m.mirroring(), Mirroring::FourScreen);
    }

    #[test]
    fn irq_counter_reaches_zero_then_asserts_irq() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            false,
        );
        // Latch=3, force reload, enable IRQ.
        m.cpu_write(0xC000, 3); // latch
        m.cpu_write(0xC001, 0); // request reload
        m.cpu_write(0xE001, 0); // enable

        // First pulse: counter is 0 + reload pending → loads from latch (3).
        pulse_a12(&mut m);
        assert!(!m.irq_pending());
        // Second pulse: 3 → 2.
        pulse_a12(&mut m);
        assert!(!m.irq_pending());
        // Third: 2 → 1.
        pulse_a12(&mut m);
        assert!(!m.irq_pending());
        // Fourth: 1 → 0 → IRQ asserts.
        pulse_a12(&mut m);
        assert!(m.irq_pending());

        // IRQ stays asserted until acknowledged via $E000.
        pulse_a12(&mut m);
        assert!(m.irq_pending());

        m.cpu_write(0xE000, 0); // disable + acknowledge
        assert!(!m.irq_pending());
    }

    #[test]
    fn irq_does_not_fire_when_disabled() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            false,
        );
        m.cpu_write(0xC000, 1);
        m.cpu_write(0xC001, 0);
        // IRQ left disabled.
        pulse_a12(&mut m); // reload
        pulse_a12(&mut m); // 1 → 0, but disabled
        assert!(!m.irq_pending());
    }

    #[test]
    fn prg_ram_protect_disables_writes_and_reads() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            true,
        );
        // Enable PRG-RAM (bit 7), allow writes (bit 6 clear).
        m.cpu_write(0xA001, 0x80);
        m.cpu_write(0x6000, 0x42);
        assert_eq!(m.cpu_read(0x6000), 0x42);

        // Enable + write-protect (bit 6 set): writes drop, reads still work.
        m.cpu_write(0xA001, 0xC0);
        m.cpu_write(0x6000, 0x99);
        assert_eq!(m.cpu_read(0x6000), 0x42);

        // Disable entirely: reads return 0.
        m.cpu_write(0xA001, 0x00);
        assert_eq!(m.cpu_read(0x6000), 0);
    }

    #[test]
    fn battery_round_trip() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            true,
        );
        m.cpu_write(0xA001, 0x80); // enable RAM, writes allowed
        m.cpu_write(0x6000, 0x77);
        let snap = m.battery_ram().expect("battery snapshot expected").to_vec();
        let mut m2 = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            true,
        );
        m2.cpu_write(0xA001, 0x80);
        m2.load_battery(&snap);
        assert_eq!(m2.cpu_read(0x6000), 0x77);
    }

    #[test]
    fn ppu_reads_below_1000_do_not_clock_irq() {
        let mut m = Mapper004::new(
            prg_8k_markers(4),
            chr_1k_markers(8),
            Mirroring::Horizontal,
            false,
            false,
        );
        m.cpu_write(0xC000, 1);
        m.cpu_write(0xC001, 0);
        m.cpu_write(0xE001, 0);
        // Many reads without crossing A12 should not clock the counter.
        for _ in 0..32 {
            let _ = m.ppu_read(0x0000);
        }
        assert!(!m.irq_pending());
    }
}
