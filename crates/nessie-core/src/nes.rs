//! Top-level public facade for `nessie-core`.
//!
//! The [`Nes`] struct stitches the [`Cpu`](crate::cpu::Cpu), [`Ppu`],
//! [`Apu`], cartridge / [`Mapper`](crate::cart::Mapper), and two
//! [`Controller`]s into the single object hosts embed. The public surface
//! mirrors spec §5.4: `from_ines`, `step_frame`, `framebuffer`, `drain_audio`,
//! `set_button`, `reset`, `load_battery`, `battery_snapshot`, `has_battery`,
//! `cartridge_info`.
//!
//! The facade does **not** spawn threads, talk to audio devices, or render
//! anything; that lives in `nessie-runtime`. The frame loop is fully
//! synchronous: one call to [`Nes::step_frame`] runs the CPU/PPU/APU just
//! long enough for the PPU to complete exactly one NTSC frame.

use crate::apu::Apu;
use crate::bus::{NesBus, CPU_RAM_SIZE};
use crate::cart::{parse_ines, Cartridge, CartridgeInfo};
use crate::controller::{Button, Controller, Player};
use crate::cpu::Cpu;
use crate::error::CoreError;
use crate::ppu::{Ppu, FRAMEBUFFER_BYTES};

/// The full NES emulator state.
pub struct Nes {
    cpu: Cpu,
    cart: Cartridge,
    ram: Box<[u8; CPU_RAM_SIZE]>,
    ppu: Ppu,
    apu: Apu,
    controllers: [Controller; 2],
    nmi_pending: bool,
    irq_pending: bool,
}

impl std::fmt::Debug for Nes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Nes")
            .field("cpu", &self.cpu)
            .field("cart", &self.cart)
            .field("ppu_frame", &self.ppu.frame_count())
            .field("ppu_pos", &self.ppu.position())
            .field("apu_samples", &self.apu.buffered_samples())
            .field("nmi_pending", &self.nmi_pending)
            .field("irq_pending", &self.irq_pending)
            .finish()
    }
}

impl Nes {
    /// Build a fresh `Nes` from a slice containing an iNES (or tolerant
    /// NES 2.0) ROM. The CPU is hard-reset (PC pulled from the reset vector)
    /// before the first frame runs.
    pub fn from_ines(bytes: &[u8]) -> Result<Self, CoreError> {
        let cart = parse_ines(bytes)?;
        let mut nes = Self {
            cpu: Cpu::new(),
            cart,
            ram: Box::new([0u8; CPU_RAM_SIZE]),
            ppu: Ppu::new(),
            apu: Apu::new(),
            controllers: [Controller::new(); 2],
            nmi_pending: false,
            irq_pending: false,
        };
        nes.reset();
        Ok(nes)
    }

    /// Static metadata of the loaded cartridge (mapper number, sizes,
    /// SHA-1, etc.). Cloned because hosts persist it independently of the
    /// `Nes`.
    pub fn cartridge_info(&self) -> CartridgeInfo {
        self.cart.info().clone()
    }

    /// `true` iff the cartridge header advertises battery-backed PRG-RAM.
    pub fn has_battery(&self) -> bool {
        self.cart.info().has_battery
    }

    /// Take a defensive copy of the cartridge's battery-backed PRG-RAM, if
    /// any. Returns `None` for non-battery cartridges. The host persists
    /// these bytes alongside the SHA-1 of the ROM (spec §4.3).
    pub fn battery_snapshot(&self) -> Option<Vec<u8>> {
        self.cart.mapper().battery_ram().map(<[u8]>::to_vec)
    }

    /// Restore battery-backed PRG-RAM from a previous session. No-op for
    /// non-battery cartridges or when the input length does not match.
    pub fn load_battery(&mut self, bytes: &[u8]) {
        self.cart.mapper_mut().load_battery(bytes);
    }

    /// Hard-reset the CPU: re-pulls the reset vector and resets the PPU/APU
    /// to their power-on states. Cartridge RAM (battery saves) is preserved.
    pub fn reset(&mut self) {
        self.ppu.reset();
        self.apu.reset();
        self.nmi_pending = false;
        self.irq_pending = false;
        let mut extra_cycles: u32 = 0;
        let Self {
            cpu,
            cart,
            ram,
            ppu,
            apu,
            controllers,
            nmi_pending,
            irq_pending,
        } = self;
        let mut bus = NesBus {
            ram,
            cart,
            ppu,
            apu,
            controllers,
            nmi_pending,
            irq_pending,
            extra_cycles: &mut extra_cycles,
        };
        cpu.reset(&mut bus);
    }

    /// Set the pressed/released state of a single button on one controller.
    ///
    /// The change is visible to the next `$4016` / `$4017` read inside the
    /// emulation loop, so calling this between frames is sufficient for
    /// interactive responsiveness at 60 Hz.
    pub fn set_button(&mut self, player: Player, button: Button, pressed: bool) {
        self.controllers[player.index()].set_button(button, pressed);
    }

    /// 256×240 RGBA8 framebuffer of the most recently rendered frame.
    #[inline]
    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_BYTES] {
        self.ppu.framebuffer()
    }

    /// Drain accumulated mono `f32` audio samples (at the APU's configured
    /// sample rate, by default 44.1 kHz) into `out`. The internal buffer is
    /// emptied so the host can call this every frame without growing memory.
    pub fn drain_audio(&mut self, out: &mut Vec<f32>) {
        self.apu.drain_samples(out);
    }

    /// Run the CPU/PPU/APU just long enough to complete exactly one NTSC
    /// frame (the PPU's `frame_count` increments by one).
    ///
    /// In practice this is around 29,780.5 CPU cycles. Because the loop is
    /// driven by the PPU's frame counter, occasional cycle-count drift
    /// (e.g. OAM DMA writes) self-corrects on the next frame.
    pub fn step_frame(&mut self) {
        let start = self.ppu.frame_count();
        // Bound the inner loop defensively: a well-behaved ROM completes a
        // frame in ~30k CPU cycles; if we run wildly past that we'd rather
        // bail than hang the caller forever.
        let mut safety_cycles: u32 = 0;
        const SAFETY_LIMIT: u32 = 200_000;
        while self.ppu.frame_count() == start && safety_cycles < SAFETY_LIMIT {
            let cycles = self.step_one_instruction();
            safety_cycles = safety_cycles.saturating_add(cycles);
        }
    }

    /// Execute exactly one CPU instruction (or service one pending
    /// interrupt) and tick the PPU/APU/mapper by the same number of cycles.
    /// Returns the total cycle count consumed (including OAM DMA stalls).
    fn step_one_instruction(&mut self) -> u32 {
        let mut extra_cycles: u32 = 0;
        let cycles = {
            let Self {
                cpu,
                cart,
                ram,
                ppu,
                apu,
                controllers,
                nmi_pending,
                irq_pending,
            } = self;
            let mut bus = NesBus {
                ram,
                cart,
                ppu,
                apu,
                controllers,
                nmi_pending,
                irq_pending,
                extra_cycles: &mut extra_cycles,
            };
            cpu.step(&mut bus)
        };
        let total = cycles.saturating_add(extra_cycles);
        // Advance peripherals by the same number of CPU cycles the CPU
        // observed (instruction + OAM DMA stall).
        self.ppu.step(total * 3, self.cart.mapper_mut());
        if self.ppu.take_nmi() {
            self.nmi_pending = true;
        }
        self.apu.step(total);
        self.cart.mapper_mut().step(total);
        total
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Build a minimal NROM ROM whose reset vector points at an infinite
    /// loop at `$C000`. Useful for testing facade plumbing without a real
    /// game program.
    fn idle_rom() -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1); // PRG units (16 KB)
        rom.push(1); // CHR units (8 KB)
        rom.push(0); // flags6 — mapper 0
        rom.push(0); // flags7
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        // Reset vector at $FFFC/$FFFD = $C000 (16 KB PRG, mirrored, so
        // $FFFC..$FFFD lives at PRG offset 0x3FFC).
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0xC0;
        // JMP $C000 at PRG offset 0 ($C000): 4C 00 C0
        rom[16] = 0x4C;
        rom[17] = 0x00;
        rom[18] = 0xC0;
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        rom
    }

    #[test]
    fn from_ines_reports_metadata() {
        let nes = Nes::from_ines(&idle_rom()).unwrap();
        let info = nes.cartridge_info();
        assert_eq!(info.mapper, 0);
        assert_eq!(info.prg_rom_size, 16 * 1024);
        assert_eq!(info.chr_rom_size, 8 * 1024);
        assert!(!info.has_battery);
        assert_eq!(info.sha1.len(), 40);
    }

    #[test]
    fn invalid_rom_propagates_through_core_error() {
        let err = Nes::from_ines(&[0u8; 4]).unwrap_err();
        match err {
            CoreError::InvalidRom(_) => {}
            other => panic!("expected InvalidRom, got {other:?}"),
        }
    }

    #[test]
    fn step_frame_increments_ppu_frame_count() {
        let mut nes = Nes::from_ines(&idle_rom()).unwrap();
        nes.step_frame();
        assert_eq!(nes.ppu.frame_count(), 1);
        nes.step_frame();
        assert_eq!(nes.ppu.frame_count(), 2);
    }

    #[test]
    fn drain_audio_yields_samples_after_a_frame() {
        let mut nes = Nes::from_ines(&idle_rom()).unwrap();
        nes.step_frame();
        let mut samples = Vec::new();
        nes.drain_audio(&mut samples);
        assert!(!samples.is_empty(), "APU should have produced samples");
        let mut more = Vec::new();
        nes.drain_audio(&mut more);
        assert!(more.is_empty(), "second drain should be empty");
    }

    #[test]
    fn framebuffer_is_full_size() {
        let nes = Nes::from_ines(&idle_rom()).unwrap();
        assert_eq!(nes.framebuffer().len(), FRAMEBUFFER_BYTES);
    }

    #[test]
    fn set_button_updates_controller_state() {
        let mut nes = Nes::from_ines(&idle_rom()).unwrap();
        nes.set_button(Player::One, Button::A, true);
        nes.set_button(Player::Two, Button::Start, true);
        assert_eq!(nes.controllers[0].state() & 0x01, 0x01);
        assert_eq!(nes.controllers[1].state() & 0x08, 0x08);
        nes.set_button(Player::One, Button::A, false);
        assert_eq!(nes.controllers[0].state() & 0x01, 0);
    }

    #[test]
    fn reset_pulls_reset_vector_from_cartridge() {
        let mut nes = Nes::from_ines(&idle_rom()).unwrap();
        // Move PC somewhere arbitrary and reset to confirm it's reloaded.
        nes.cpu.pc = 0x1234;
        nes.reset();
        assert_eq!(nes.cpu.pc, 0xC000);
    }
}
