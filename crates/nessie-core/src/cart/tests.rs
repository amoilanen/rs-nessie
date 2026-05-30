//! Unit tests for the iNES parser, [`Mirroring`] helper, and [`Mapper000`].
//!
//! Tests build small synthetic ROMs in-memory rather than reading from disk so
//! they stay self-contained and platform-independent.

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use super::*;

/// Build a minimal iNES header. Caller is responsible for appending PRG/CHR
/// payload bytes that match the declared sizes.
fn ines_header(prg_units: u8, chr_units: u8, flags6: u8, flags7: u8) -> Vec<u8> {
    let mut h = Vec::with_capacity(16);
    h.extend_from_slice(b"NES\x1a");
    h.push(prg_units);
    h.push(chr_units);
    h.push(flags6);
    h.push(flags7);
    h.extend_from_slice(&[0u8; 8]);
    h
}

/// 16 KB PRG + 8 KB CHR, all zero, mirroring per `flags6`.
fn nrom_16k_8k(flags6: u8) -> Vec<u8> {
    let mut rom = ines_header(1, 1, flags6, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    rom
}

#[test]
fn parses_valid_nrom_16k_8k() {
    let rom = nrom_16k_8k(0);
    let cart = parse_ines(&rom).expect("should parse");
    let info = cart.info();
    assert_eq!(info.mapper, 0);
    assert_eq!(info.prg_rom_size, 16 * 1024);
    assert_eq!(info.chr_rom_size, 8 * 1024);
    assert_eq!(info.mirroring, Mirroring::Horizontal);
    assert!(!info.has_battery);
}

#[test]
fn parses_nrom_32k_with_chr_ram() {
    // PRG=2 (32 KB), CHR=0 (use CHR-RAM), vertical mirroring.
    let mut rom = ines_header(2, 0, 0x01, 0);
    rom.extend(std::iter::repeat(0xAA).take(32 * 1024));
    let cart = parse_ines(&rom).expect("should parse");
    assert_eq!(cart.info().prg_rom_size, 32 * 1024);
    assert_eq!(cart.info().chr_rom_size, 0);
    assert_eq!(cart.info().mirroring, Mirroring::Vertical);
}

#[test]
fn rejects_short_buffer() {
    let bytes = vec![0u8; 8];
    assert!(matches!(parse_ines(&bytes), Err(ParseError::TooShort(8))));
}

#[test]
fn rejects_missing_magic() {
    let mut rom = nrom_16k_8k(0);
    rom[0] = b'X'; // corrupt the magic
    assert_eq!(parse_ines(&rom).err(), Some(ParseError::InvalidMagic));
}

#[test]
fn rejects_zero_prg_units() {
    // PRG=0 is degenerate; the cart would have no code to execute.
    let rom = ines_header(0, 1, 0, 0);
    assert_eq!(parse_ines(&rom).err(), Some(ParseError::NoPrgRom));
}

#[test]
fn rejects_truncated_body() {
    // Header advertises 16 KB PRG + 8 KB CHR but body is only half present.
    let mut rom = ines_header(1, 1, 0, 0);
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let err = parse_ines(&rom).expect_err("should fail");
    match err {
        ParseError::TruncatedBody { expected, actual } => {
            assert!(expected > actual, "expected={expected} actual={actual}");
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn unsupported_mapper_number() {
    // Mapper numbers outside `{0,1,2,3,4}` are rejected. Encode mapper 5
    // (MMC5) by placing 5 in the low nibble of flags6 (bits 4-7).
    let mut rom = ines_header(1, 1, 0x50, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    assert_eq!(
        parse_ines(&rom).err(),
        Some(ParseError::UnsupportedMapper(5))
    );
}

#[test]
fn sha1_matches_known_digest() {
    // Build a deterministic 24 KB body ROM. The expected SHA-1 was computed
    // independently with `python3 -c "hashlib.sha1(bytes).hexdigest()"`.
    let mut rom = ines_header(1, 1, 0, 0);
    rom.extend((0..16384u32).map(|i| (i & 0xFF) as u8));
    rom.extend((0..8192u32).map(|i| ((i + 7) & 0xFF) as u8));
    assert_eq!(rom.len(), 16 + 16 * 1024 + 8 * 1024);
    let cart = parse_ines(&rom).expect("should parse");
    assert_eq!(cart.sha1(), "8e091314aeaa66488ca6b572a20352c1a47333c2");
}

#[test]
fn detects_battery_flag() {
    // flags6 bit 1 = battery, bit 0 = vertical.
    let mut rom = ines_header(1, 1, 0x02, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let cart = parse_ines(&rom).expect("should parse");
    assert!(cart.info().has_battery);
}

#[test]
fn skips_trainer_when_present() {
    // flags6 bit 2 = 512-byte trainer between header and PRG.
    let mut rom = ines_header(1, 1, 0x04, 0);
    // Trainer: 512 bytes of 0xEE, then PRG of 0xAA, then CHR of 0xBB.
    rom.extend(std::iter::repeat(0xEEu8).take(512));
    rom.extend(std::iter::repeat(0xAAu8).take(16 * 1024));
    rom.extend(std::iter::repeat(0xBBu8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    // After parsing, PRG-ROM should be 0xAA (NOT the trainer's 0xEE).
    assert_eq!(cart.mapper_mut().cpu_read(0x8000), 0xAA);
    assert_eq!(cart.mapper_mut().ppu_read(0x0000), 0xBB);
}

#[test]
fn mapper_000_mirrors_16k_prg_into_top_window() {
    // 16 KB PRG where each byte equals its low-8-bits offset; verify that
    // $C000-$FFFF mirrors $8000-$BFFF.
    let mut rom = ines_header(1, 1, 0, 0);
    rom.extend((0..16384u32).map(|i| (i & 0xFF) as u8));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    assert_eq!(m.cpu_read(0x8000), 0x00);
    assert_eq!(m.cpu_read(0xC000), 0x00); // mirror
    assert_eq!(m.cpu_read(0x8001), 0x01);
    assert_eq!(m.cpu_read(0xC001), 0x01);
    assert_eq!(m.cpu_read(0xBFFF), 0xFF);
    assert_eq!(m.cpu_read(0xFFFF), 0xFF);
}

#[test]
fn mapper_000_does_not_mirror_32k_prg() {
    // First 16 KB bank filled with 0x11, second 16 KB bank filled with 0x22.
    // A correctly-wired 32 KB cart returns 0x11 at $8000-$BFFF and 0x22 at
    // $C000-$FFFF (no mirroring of the low bank into the high window).
    let mut rom = ines_header(2, 1, 0, 0);
    rom.extend(std::iter::repeat(0x11u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0x22u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    assert_eq!(m.cpu_read(0x8000), 0x11);
    assert_eq!(m.cpu_read(0xBFFF), 0x11);
    assert_eq!(m.cpu_read(0xC000), 0x22);
    assert_eq!(m.cpu_read(0xFFFF), 0x22);
}

#[test]
fn mapper_000_returns_chr_bytes_on_ppu_read() {
    let mut rom = ines_header(1, 1, 0, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend((0..8192u32).map(|i| (i & 0xFF) as u8));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    assert_eq!(m.ppu_read(0x0000), 0x00);
    assert_eq!(m.ppu_read(0x00FF), 0xFF);
    assert_eq!(m.ppu_read(0x1FFF), 0xFF);
}

#[test]
fn mapper_000_rejects_prg_rom_writes_silently() {
    // Writing to PRG-ROM space must be a no-op (no panic, no state change).
    let mut rom = ines_header(1, 1, 0, 0);
    rom.extend((0..16384u32).map(|i| (i & 0xFF) as u8));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    let before = m.cpu_read(0x8000);
    m.cpu_write(0x8000, 0x55);
    let after = m.cpu_read(0x8000);
    assert_eq!(before, after, "PRG-ROM write must not change ROM contents");
}

#[test]
fn mapper_000_chr_rom_writes_are_ignored() {
    let mut rom = ines_header(1, 1, 0, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0x77u8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    assert_eq!(m.ppu_read(0x0000), 0x77);
    m.ppu_write(0x0000, 0xAB);
    assert_eq!(m.ppu_read(0x0000), 0x77, "CHR-ROM writes must be ignored");
}

#[test]
fn mapper_000_chr_ram_writes_round_trip() {
    // CHR units = 0 → CHR-RAM. Writes via the PPU bus should persist.
    let mut rom = ines_header(1, 0, 0, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    m.ppu_write(0x0123, 0x99);
    assert_eq!(m.ppu_read(0x0123), 0x99);
}

#[test]
fn mapper_000_prg_ram_round_trips_and_battery_snapshot() {
    // Battery flag set; PRG-RAM writes survive and are visible via battery_ram().
    let mut rom = ines_header(1, 1, 0x02, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    m.cpu_write(0x6000, 0xDE);
    m.cpu_write(0x7FFF, 0xAD);
    assert_eq!(m.cpu_read(0x6000), 0xDE);
    assert_eq!(m.cpu_read(0x7FFF), 0xAD);
    let snap = m.battery_ram().expect("battery snapshot expected");
    assert_eq!(snap.len(), 8 * 1024);
    assert_eq!(snap[0], 0xDE);
    assert_eq!(snap[0x1FFF], 0xAD);
}

#[test]
fn mapper_000_load_battery_restores_prg_ram() {
    let mut rom = ines_header(1, 1, 0x02, 0);
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    let mut cart = parse_ines(&rom).expect("should parse");
    let m = cart.mapper_mut();
    let mut saved = vec![0u8; 8 * 1024];
    saved[0] = 0x11;
    saved[42] = 0x42;
    m.load_battery(&saved);
    assert_eq!(m.cpu_read(0x6000), 0x11);
    assert_eq!(m.cpu_read(0x6000 + 42), 0x42);
}

#[test]
fn mapper_000_without_battery_returns_none() {
    let cart = parse_ines(&nrom_16k_8k(0)).expect("should parse");
    assert!(cart.mapper().battery_ram().is_none());
}

#[test]
fn nes20_header_extends_mapper_number() {
    // NES 2.0 marker: byte 7 bits 2-3 = 0b10 → 0x08. Byte 8 low nibble = mapper hi.
    // Set mapper bits: lo = 0, mid = 0, hi (byte8) = 0x0F → mapper number 0xF00 = 3840.
    let mut rom = ines_header(1, 1, 0x00, 0x08);
    rom[8] = 0x0F;
    rom.extend(std::iter::repeat(0u8).take(16 * 1024));
    rom.extend(std::iter::repeat(0u8).take(8 * 1024));
    assert_eq!(
        parse_ines(&rom).err(),
        Some(ParseError::UnsupportedMapper(0xF00))
    );
}

#[test]
fn mirroring_horizontal_nametable_index() {
    // Horizontal arrangement: NT0/NT1 → bank 0, NT2/NT3 → bank 1.
    assert_eq!(Mirroring::Horizontal.nametable_index(0x2000), 0);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x23FF), 0);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x2400), 0);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x27FF), 0);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x2800), 1);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x2BFF), 1);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x2C00), 1);
    assert_eq!(Mirroring::Horizontal.nametable_index(0x2FFF), 1);
}

#[test]
fn mirroring_vertical_nametable_index() {
    // Vertical arrangement: NT0/NT2 → bank 0, NT1/NT3 → bank 1.
    assert_eq!(Mirroring::Vertical.nametable_index(0x2000), 0);
    assert_eq!(Mirroring::Vertical.nametable_index(0x2400), 1);
    assert_eq!(Mirroring::Vertical.nametable_index(0x2800), 0);
    assert_eq!(Mirroring::Vertical.nametable_index(0x2C00), 1);
}

#[test]
fn mirroring_four_screen_nametable_index() {
    // Four-screen: each logical nametable maps 1:1 to a physical one.
    assert_eq!(Mirroring::FourScreen.nametable_index(0x2000), 0);
    assert_eq!(Mirroring::FourScreen.nametable_index(0x2400), 1);
    assert_eq!(Mirroring::FourScreen.nametable_index(0x2800), 2);
    assert_eq!(Mirroring::FourScreen.nametable_index(0x2C00), 3);
}

#[test]
fn mirroring_one_screen_lower_nametable_index() {
    for addr in [0x2000u16, 0x2400, 0x2800, 0x2C00] {
        assert_eq!(Mirroring::OneScreenLower.nametable_index(addr), 0);
    }
}

#[test]
fn mirroring_one_screen_upper_nametable_index() {
    for addr in [0x2000u16, 0x2400, 0x2800, 0x2C00] {
        assert_eq!(Mirroring::OneScreenUpper.nametable_index(addr), 1);
    }
}
