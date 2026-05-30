//! Unit tests for the PPU.
//!
//! The synthetic mappers used here mirror real cartridge behaviour just
//! enough to exercise the PPU's bus interactions without dragging in the
//! iNES parser.

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use super::*;
use crate::cart::{Mapper, Mirroring};
use sha1::{Digest, Sha1};

/// Minimal in-memory mapper: 8 KB CHR-RAM, configurable mirroring, no PRG.
/// Used to exercise the PPU's CHR pattern fetches and nametable mirroring.
struct TestMapper {
    chr: [u8; 0x2000],
    mirroring: Mirroring,
}

impl TestMapper {
    fn new(mirroring: Mirroring) -> Self {
        Self {
            chr: [0u8; 0x2000],
            mirroring,
        }
    }
}

impl Mapper for TestMapper {
    fn cpu_read(&mut self, _addr: u16) -> u8 {
        0
    }
    fn cpu_write(&mut self, _addr: u16, _value: u8) {}
    fn ppu_read(&mut self, addr: u16) -> u8 {
        self.chr[(addr & 0x1FFF) as usize]
    }
    fn ppu_write(&mut self, addr: u16, value: u8) {
        self.chr[(addr & 0x1FFF) as usize] = value;
    }
    fn mirroring(&self) -> Mirroring {
        self.mirroring
    }
}

// -----------------------------------------------------------------------
// PPUSTATUS / vblank timing.
// -----------------------------------------------------------------------

#[test]
fn vblank_flag_sets_at_scanline_241_dot_1() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // Position machine: after `step(N_cpu)` the PPU has performed `3*N_cpu`
    // ticks, each consuming one PPU dot. The vblank-set event fires inside
    // the tick whose *before-pos* is (scanline=241, cycle=1) — i.e. after
    // exactly `241*341 + 1 + 1 = 82183` dots have begun. So:
    //   - 82182 dots ⇒ before-pos is (241, 0): not yet set.
    //   - 82185 dots ⇒ before-pos in {(241,0),(241,1),(241,2)}: set.
    // 82182 dots = 27394 CPU cycles; 82185 dots = 27395 CPU cycles.
    ppu.step(27394, &mut mapper);
    assert_eq!(ppu.read_register(0x2002, &mut mapper) & VBLANK, 0);

    ppu.step(1, &mut mapper);
    assert_ne!(ppu.read_register(0x2002, &mut mapper) & VBLANK, 0);
}

#[test]
fn vblank_nmi_fires_when_ctrl_bit7_set() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);
    ppu.write_register(0x2000, 0x80, &mut mapper); // enable NMI

    // Step to a point well inside vblank but before the pre-render scanline
    // clears the flag (84000 dots = 28000 CPU cycles).
    ppu.step(28000, &mut mapper);
    assert!(ppu.take_nmi(), "NMI should have fired at vblank entry");
    // Edge-triggered: subsequent reads return false until next vblank.
    assert!(!ppu.take_nmi());
}

#[test]
fn vblank_nmi_does_not_fire_when_ctrl_bit7_clear() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);
    ppu.step(28000, &mut mapper);
    assert!(!ppu.take_nmi());
}

#[test]
fn reading_status_clears_vblank_and_write_toggle() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);
    // Step into vblank but not as far as the pre-render scanline that
    // would clear the flag again. 28000 CPU cycles = 84000 PPU dots, which
    // lies between vblank entry (82183 dots) and pre-render clear (89003).
    ppu.step(28000, &mut mapper);
    // Engage the toggle via a PPUADDR write.
    ppu.write_register(0x2006, 0x21, &mut mapper);
    assert!(ppu.debug_w());
    let status = ppu.read_register(0x2002, &mut mapper);
    assert_ne!(status & VBLANK, 0);
    assert!(!ppu.debug_w());
    // Subsequent read shows vblank cleared.
    let status2 = ppu.read_register(0x2002, &mut mapper);
    assert_eq!(status2 & VBLANK, 0);
}

// -----------------------------------------------------------------------
// PPUADDR write toggle and v/t loopy-register updates.
// -----------------------------------------------------------------------

#[test]
fn ppuaddr_write_sequence_updates_v_and_t() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // After reset the write toggle is at the "first write" state.
    assert!(!ppu.debug_w());

    // First write to $2006 sets high byte: t bits 8..=13 = $21 & 0x3F = 0x21.
    ppu.write_register(0x2006, 0x21, &mut mapper);
    assert!(ppu.debug_w());
    assert_eq!(ppu.debug_t() & 0xFF00, 0x2100);
    // v is not updated by the first write.
    assert_eq!(ppu.debug_v(), 0);

    // Second write sets low byte and copies t→v: result address is $2108.
    ppu.write_register(0x2006, 0x08, &mut mapper);
    assert!(!ppu.debug_w());
    assert_eq!(ppu.debug_t(), 0x2108);
    assert_eq!(ppu.debug_v(), 0x2108);
}

#[test]
fn ppuscroll_writes_set_coarse_fine_and_toggle() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // First write to $2005: X scroll = 0x7D = 0b01111_101.
    //   coarse X = 0x0F into t[0..=4]; fine X = 5 into self.x.
    ppu.write_register(0x2005, 0x7D, &mut mapper);
    assert!(ppu.debug_w());
    assert_eq!(ppu.debug_t() & 0x001F, 0x000F);
    assert_eq!(ppu.debug_x(), 5);

    // Second write to $2005: Y scroll = 0x5E = 0b01011_110.
    //   coarse Y = 0x0B into t[5..=9]; fine Y = 6 into t[12..=14].
    ppu.write_register(0x2005, 0x5E, &mut mapper);
    assert!(!ppu.debug_w());
    let expected_t = (0x000Fu16) | (0x000Bu16 << 5) | (0x0006u16 << 12);
    assert_eq!(ppu.debug_t(), expected_t);
}

#[test]
fn ppuctrl_sets_nametable_bits_of_t() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);
    ppu.write_register(0x2000, 0x03, &mut mapper);
    // t bits 10..=11 should be 0b11.
    assert_eq!(ppu.debug_t() & 0x0C00, 0x0C00);
}

// -----------------------------------------------------------------------
// Palette RAM mirroring.
// -----------------------------------------------------------------------

#[test]
fn palette_mirrors_3f10_3f14_3f18_3f1c() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    fn write(ppu: &mut Ppu, mapper: &mut TestMapper, addr: u16, val: u8) {
        ppu.write_register(0x2006, (addr >> 8) as u8, mapper);
        ppu.write_register(0x2006, addr as u8, mapper);
        ppu.write_register(0x2007, val, mapper);
    }
    fn read(ppu: &mut Ppu, mapper: &mut TestMapper, addr: u16) -> u8 {
        // Palette range bypasses the read buffer, so one read returns the
        // actual value.
        ppu.write_register(0x2006, (addr >> 8) as u8, mapper);
        ppu.write_register(0x2006, addr as u8, mapper);
        ppu.read_register(0x2007, mapper)
    }

    write(&mut ppu, &mut mapper, 0x3F00, 0x11);
    assert_eq!(read(&mut ppu, &mut mapper, 0x3F10), 0x11);

    write(&mut ppu, &mut mapper, 0x3F14, 0x22);
    assert_eq!(read(&mut ppu, &mut mapper, 0x3F04), 0x22);

    write(&mut ppu, &mut mapper, 0x3F18, 0x33);
    assert_eq!(read(&mut ppu, &mut mapper, 0x3F08), 0x33);

    write(&mut ppu, &mut mapper, 0x3F1C, 0x44);
    assert_eq!(read(&mut ppu, &mut mapper, 0x3F0C), 0x44);

    // Other palette addresses are not mirrored: $3F01 != $3F11.
    write(&mut ppu, &mut mapper, 0x3F01, 0x55);
    assert_ne!(read(&mut ppu, &mut mapper, 0x3F11), 0x55);
}

// -----------------------------------------------------------------------
// Nametable mirroring.
// -----------------------------------------------------------------------

#[test]
fn nametable_horizontal_mirroring_routes_pairs() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // Write distinct bytes into NT0 ($2000) and NT2 ($2800). With horizontal
    // mirroring NT0↔NT1 share one bank and NT2↔NT3 share the other, so
    // $2400 == $2000 and $2C00 == $2800.
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    ppu.write_register(0x2007, 0xAA, &mut mapper);

    ppu.write_register(0x2006, 0x28, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    ppu.write_register(0x2007, 0xBB, &mut mapper);

    assert_eq!(ppu.debug_peek(0x2000, &mut mapper), 0xAA);
    assert_eq!(ppu.debug_peek(0x2400, &mut mapper), 0xAA);
    assert_eq!(ppu.debug_peek(0x2800, &mut mapper), 0xBB);
    assert_eq!(ppu.debug_peek(0x2C00, &mut mapper), 0xBB);
}

// -----------------------------------------------------------------------
// VRAM increment mode.
// -----------------------------------------------------------------------

#[test]
fn ppudata_writes_use_vram_increment_mode() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // Default increment is +1.
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    ppu.write_register(0x2007, 0x01, &mut mapper);
    ppu.write_register(0x2007, 0x02, &mut mapper);
    assert_eq!(ppu.debug_peek(0x2000, &mut mapper), 0x01);
    assert_eq!(ppu.debug_peek(0x2001, &mut mapper), 0x02);

    // Switch to +32 mode.
    ppu.write_register(0x2000, 0x04, &mut mapper);
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x40, &mut mapper);
    ppu.write_register(0x2007, 0x55, &mut mapper);
    ppu.write_register(0x2007, 0x66, &mut mapper);
    assert_eq!(ppu.debug_peek(0x2040, &mut mapper), 0x55);
    assert_eq!(ppu.debug_peek(0x2060, &mut mapper), 0x66);
}

// -----------------------------------------------------------------------
// Sprite-zero hit.
// -----------------------------------------------------------------------

#[test]
fn sprite_zero_hit_set_when_opaque_pixels_overlap() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // CHR setup:
    // - Background tile 1 at $0000-$000F: all pixels of color 1.
    //   8 rows, plane 0 = 0xFF, plane 1 = 0x00.
    for row in 0..8 {
        mapper.ppu_write(0x0010 + row, 0xFF);
        mapper.ppu_write(0x0018 + row, 0x00);
    }
    // - Sprite tile 1 at $1000-$100F: all pixels of color 1.
    for row in 0..8 {
        mapper.ppu_write(0x1010 + row, 0xFF);
        mapper.ppu_write(0x1018 + row, 0x00);
    }

    // Fill the entire nametable 0 with tile index 1 so every BG pixel is
    // opaque.
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    for _ in 0..(32 * 30) {
        ppu.write_register(0x2007, 0x01, &mut mapper);
    }
    // Attribute table — leave palette 0 selected (already zero).

    // Populate palette so color index 1 is a non-transparent visible color.
    ppu.write_register(0x2006, 0x3F, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    ppu.write_register(0x2007, 0x0F, &mut mapper); // bg color 0
    ppu.write_register(0x2007, 0x21, &mut mapper); // bg color 1
                                                   // Set sprite palette 0 colors.
    ppu.write_register(0x2006, 0x3F, &mut mapper);
    ppu.write_register(0x2006, 0x10, &mut mapper);
    ppu.write_register(0x2007, 0x0F, &mut mapper);
    ppu.write_register(0x2007, 0x30, &mut mapper);

    // Place sprite 0 at (x=16, y=15). With y=15 the sprite occupies screen
    // rows 16..=23, well clear of the top-left mask zone (rows 0..=7) and not
    // on x=255. Pattern uses sprite-pattern table $1000 (PPUCTRL bit 3).
    {
        let oam = ppu.debug_oam_mut();
        oam[0] = 15; // Y
        oam[1] = 0x01; // tile
        oam[2] = 0x00; // attr (palette 0, front, no flip)
        oam[3] = 16; // X
    }

    // Reset the loopy v/t registers to point back at the start of NT0
    // (PPUADDR writes during palette setup left v at $3F12 otherwise).
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);

    // Enable rendering (BG + sprites, both including the leftmost 8 columns)
    // and select sprite pattern table $1000.
    ppu.write_register(0x2000, 0x08, &mut mapper);
    ppu.write_register(0x2001, 0x1E, &mut mapper);

    // Run until partway through vblank (84000 dots = 28000 CPU cycles).
    // Stopping before the pre-render scanline preserves the sprite-0-hit
    // flag that was set during scanline 16.
    ppu.step(28000, &mut mapper);

    let status = ppu.read_register(0x2002, &mut mapper);
    assert_ne!(
        status & SPRITE_ZERO_HIT,
        0,
        "sprite 0 hit should be set after a frame with overlapping opaque pixels"
    );
}

// Replacing the placeholder palette-write at $3F00 helper inadvertently
// shadowed earlier writes. This separate test confirms the two-write
// sequence we depend on really does land in palette RAM.
#[test]
fn palette_write_sequence_lands_in_palette_ram() {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);
    ppu.write_register(0x2006, 0x3F, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    ppu.write_register(0x2007, 0x12, &mut mapper);
    ppu.write_register(0x2007, 0x34, &mut mapper);

    ppu.write_register(0x2006, 0x3F, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    // Palette read returns the actual byte (no buffer delay).
    assert_eq!(ppu.read_register(0x2007, &mut mapper), 0x12);
    assert_eq!(ppu.read_register(0x2007, &mut mapper), 0x34);
}

// -----------------------------------------------------------------------
// Deterministic frame hash.
// -----------------------------------------------------------------------

/// SHA-1 of the framebuffer produced by [`build_deterministic_frame`].
///
/// This value is generated by running the test once, snapshotting the hash,
/// and committing it. A regression in the PPU rendering math will cause this
/// test to fail with a clear before/after comparison.
const DETERMINISTIC_FRAME_SHA1: &str = "6f02aefe09bf8459249dee805d7f3b3484a2c102";

fn build_deterministic_frame() -> Ppu {
    let mut ppu = Ppu::new();
    let mut mapper = TestMapper::new(Mirroring::Horizontal);

    // Tile 1: vertical stripe pattern — plane 0 = 0xAA, plane 1 = 0x55, so
    // each row produces the pixel sequence 1, 2, 1, 2, 1, 2, 1, 2.
    for row in 0..8 {
        mapper.ppu_write(0x0010 + row, 0xAA);
        mapper.ppu_write(0x0018 + row, 0x55);
    }
    // Tile 2: solid color 3.
    for row in 0..8 {
        mapper.ppu_write(0x0020 + row, 0xFF);
        mapper.ppu_write(0x0028 + row, 0xFF);
    }

    // Fill nametable 0 with alternating tiles to produce a checkerboard.
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);
    for row in 0..30 {
        for col in 0..32 {
            let tile = if (row + col) & 1 == 0 { 0x01 } else { 0x02 };
            ppu.write_register(0x2007, tile, &mut mapper);
        }
    }

    // Set palette entries.
    let palette_writes: &[(u16, u8)] = &[
        (0x3F00, 0x0F),
        (0x3F01, 0x21),
        (0x3F02, 0x2A),
        (0x3F03, 0x16),
    ];
    for (addr, val) in palette_writes {
        ppu.write_register(0x2006, (*addr >> 8) as u8, &mut mapper);
        ppu.write_register(0x2006, (*addr & 0xFF) as u8, &mut mapper);
        ppu.write_register(0x2007, *val, &mut mapper);
    }

    // Reset v back to $2000 so rendering starts at NT0 top-left.
    ppu.write_register(0x2006, 0x20, &mut mapper);
    ppu.write_register(0x2006, 0x00, &mut mapper);

    // Enable BG rendering.
    ppu.write_register(0x2001, 0x0A, &mut mapper);

    // Run a single frame.
    ppu.step(29781, &mut mapper);
    ppu
}

#[test]
fn deterministic_frame_hash_matches_committed_digest() {
    let ppu = build_deterministic_frame();
    let mut hasher = Sha1::new();
    hasher.update(ppu.framebuffer().as_ref());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    if DETERMINISTIC_FRAME_SHA1 == "PLACEHOLDER" {
        // Bootstrapping path: print the hash so the developer can paste it
        // into `DETERMINISTIC_FRAME_SHA1`. Fail loudly to make sure CI never
        // accepts a placeholder.
        panic!("deterministic frame hash placeholder; observed digest = {hex}");
    }
    assert_eq!(
        hex, DETERMINISTIC_FRAME_SHA1,
        "deterministic frame hash drifted (rendering regression?)"
    );
}
