//! Cartridge module: iNES parser, mapper trait, and concrete mapper implementations.
//!
//! The [`Cartridge`] type is the runtime representation of a loaded ROM. It owns
//! a boxed [`Mapper`] that the bus and PPU drive on every CPU/PPU memory access.
//!
//! The parser accepts both classic iNES 1.0 and NES 2.0 headers tolerantly: the
//! extra NES 2.0 metadata is read when present, but a strict NES 2.0 validator is
//! out of scope for this step. Malformed headers (bad magic, zero PRG-ROM units,
//! truncated body) are rejected with [`ParseError`], which the host layer maps
//! into `CoreError::InvalidRom` per the spec.

use sha1::{Digest, Sha1};
use thiserror::Error;

mod mapper_000;
mod mapper_001;
mod mapper_002;
mod mapper_003;
mod mapper_004;

pub use mapper_000::Mapper000;
pub use mapper_001::Mapper001;
pub use mapper_002::Mapper002;
pub use mapper_003::Mapper003;
pub use mapper_004::Mapper004;

#[cfg(test)]
mod tests;

/// Nametable mirroring mode.
///
/// The NES has 2 KB of nametable RAM but addresses four logical nametables.
/// The cartridge wires the upper bits of the PPU address to physical nametables
/// in one of these modes. `FourScreen` cartridges include extra nametable RAM.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mirroring {
    /// Horizontal arrangement (vertical mirroring of the address bus): NT0/NT1
    /// share one physical table, NT2/NT3 share the other.
    Horizontal,
    /// Vertical arrangement (horizontal mirroring of the address bus): NT0/NT2
    /// share one physical table, NT1/NT3 share the other.
    Vertical,
    /// Cartridge supplies extra RAM; all four logical nametables are distinct.
    FourScreen,
    /// All four logical nametables map to the cartridge's lower bank.
    OneScreenLower,
    /// All four logical nametables map to the cartridge's upper bank.
    OneScreenUpper,
}

impl Mirroring {
    /// Resolve a PPU nametable address (only the low 12 bits are used) to a
    /// physical nametable index in `0..=3`.
    ///
    /// `0x2000..=0x23FF` → logical NT0, `0x2400..=0x27FF` → NT1,
    /// `0x2800..=0x2BFF` → NT2, `0x2C00..=0x2FFF` → NT3 (the same applies for
    /// the `0x3000..=0x3EFF` mirror).
    #[inline]
    pub fn nametable_index(self, addr: u16) -> usize {
        let nt = ((addr >> 10) & 0x3) as usize;
        match self {
            Mirroring::Horizontal => [0, 0, 1, 1][nt],
            Mirroring::Vertical => [0, 1, 0, 1][nt],
            Mirroring::FourScreen => nt,
            Mirroring::OneScreenLower => 0,
            Mirroring::OneScreenUpper => 1,
        }
    }
}

/// Errors returned by [`parse_ines`].
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ParseError {
    /// ROM does not contain a full 16-byte header.
    #[error("ROM too short: {0} bytes (need at least 16)")]
    TooShort(usize),
    /// First four bytes are not the iNES magic `"NES\x1a"`.
    #[error("invalid iNES magic (expected 'NES\\x1a')")]
    InvalidMagic,
    /// Header declared zero PRG-ROM banks; the cartridge would have no code.
    #[error("PRG ROM size is zero")]
    NoPrgRom,
    /// File ends before the declared PRG/CHR payload completes.
    #[error("truncated ROM body: expected at least {expected} bytes, got {actual}")]
    TruncatedBody {
        /// Minimum byte length implied by the header.
        expected: usize,
        /// Actual byte length supplied.
        actual: usize,
    },
    /// Mapper number is recognized in iNES but not implemented yet.
    #[error("unsupported mapper {0}")]
    UnsupportedMapper(u16),
}

/// Static metadata exposed by a parsed cartridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartridgeInfo {
    /// Mapper number as parsed from the iNES header (combined upper + lower nibble).
    pub mapper: u16,
    /// PRG-ROM size in bytes (declared header units × 16 KB).
    pub prg_rom_size: usize,
    /// CHR-ROM size in bytes (declared header units × 8 KB). Zero indicates the
    /// cartridge uses CHR-RAM instead.
    pub chr_rom_size: usize,
    /// Initial nametable mirroring mode as wired by the cartridge.
    pub mirroring: Mirroring,
    /// `true` when the header advertises battery-backed PRG-RAM at `$6000-$7FFF`.
    pub has_battery: bool,
    /// SHA-1 of the original ROM bytes as a lowercase hex string. Used by the
    /// host as a stable key for battery saves.
    pub sha1: String,
}

/// Read/write interface backing the cartridge slot on the CPU and PPU buses.
///
/// Concrete mappers (NROM, MMC1, …) implement bank switching, mirroring control,
/// IRQ generation, and battery-backed RAM. Required methods cover the four bus
/// access paths plus current [`Mirroring`]; the rest have default no-op
/// implementations that suit fixed-bank mappers like NROM.
pub trait Mapper: Send {
    /// CPU read of an address in the cartridge space (`$4020-$FFFF`).
    fn cpu_read(&mut self, addr: u16) -> u8;
    /// CPU write of an address in the cartridge space.
    fn cpu_write(&mut self, addr: u16, value: u8);
    /// PPU read of an address in the pattern/nametable mirror (`$0000-$1FFF`
    /// for CHR, plus the cartridge's nametable wiring above).
    fn ppu_read(&mut self, addr: u16) -> u8;
    /// PPU write to the same range.
    fn ppu_write(&mut self, addr: u16, value: u8);
    /// Current nametable mirroring mode. Most mappers return a fixed value; MMC1
    /// and a few others mutate this in response to register writes.
    fn mirroring(&self) -> Mirroring;
    /// `true` while the mapper is asserting `/IRQ` on the CPU.
    fn irq_pending(&self) -> bool {
        false
    }
    /// Advance the mapper's internal counters by `cycles` CPU cycles. Used by
    /// MMC3-style scanline IRQ counters.
    fn step(&mut self, _cycles: u32) {}
    /// Borrowed view of battery-backed RAM if any.
    fn battery_ram(&self) -> Option<&[u8]> {
        None
    }
    /// Restore battery-backed RAM contents from a previous session.
    fn load_battery(&mut self, _bytes: &[u8]) {}
}

/// A parsed NES cartridge bundling its metadata and bus-facing [`Mapper`].
pub struct Cartridge {
    info: CartridgeInfo,
    mapper: Box<dyn Mapper>,
}

impl std::fmt::Debug for Cartridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The mapper is a trait object without a `Debug` bound; print only the
        // metadata.
        f.debug_struct("Cartridge")
            .field("info", &self.info)
            .finish()
    }
}

impl Cartridge {
    /// Read-only access to the cartridge's static metadata.
    pub fn info(&self) -> &CartridgeInfo {
        &self.info
    }

    /// SHA-1 of the original ROM bytes (lowercase hex, 40 chars).
    pub fn sha1(&self) -> &str {
        &self.info.sha1
    }

    /// Mutable access to the boxed mapper. The bus uses this on every access.
    pub fn mapper_mut(&mut self) -> &mut dyn Mapper {
        &mut *self.mapper
    }

    /// Immutable access to the boxed mapper (read-only diagnostics).
    pub fn mapper(&self) -> &dyn Mapper {
        &*self.mapper
    }

    /// Current mirroring mode (may differ from `info.mirroring` after the
    /// mapper has been written to).
    pub fn mirroring(&self) -> Mirroring {
        self.mapper.mirroring()
    }
}

/// Parse a byte slice as an iNES (or tolerant NES 2.0) ROM.
///
/// The full slice is fed to SHA-1 before any internal copies are made so the
/// returned [`Cartridge::sha1`] matches the on-disk file regardless of trainer
/// or PRG/CHR layout.
pub fn parse_ines(bytes: &[u8]) -> Result<Cartridge, ParseError> {
    if bytes.len() < 16 {
        return Err(ParseError::TooShort(bytes.len()));
    }
    if &bytes[0..4] != b"NES\x1a" {
        return Err(ParseError::InvalidMagic);
    }

    let prg_units = bytes[4] as usize;
    let chr_units = bytes[5] as usize;
    let flags6 = bytes[6];
    let flags7 = bytes[7];

    if prg_units == 0 {
        return Err(ParseError::NoPrgRom);
    }

    let has_trainer = flags6 & 0x04 != 0;
    let has_battery = flags6 & 0x02 != 0;
    let four_screen = flags6 & 0x08 != 0;
    let vertical = flags6 & 0x01 != 0;

    let mirroring = if four_screen {
        Mirroring::FourScreen
    } else if vertical {
        Mirroring::Vertical
    } else {
        Mirroring::Horizontal
    };

    // NES 2.0 detection: bits 2-3 of byte 7 must be exactly `0b10`.
    let is_nes20 = (flags7 & 0x0C) == 0x08;

    let mapper_lo = u16::from(flags6 >> 4);
    let mapper_mid = u16::from(flags7 & 0xF0);
    let mut mapper_num = mapper_mid | mapper_lo;

    if is_nes20 {
        // Byte 8 low nibble carries mapper bits 8..=11.
        let mapper_upper = u16::from(bytes[8] & 0x0F);
        mapper_num |= mapper_upper << 8;
    }

    let prg_size = prg_units * 16 * 1024;
    let chr_size = chr_units * 8 * 1024;
    let header_size = 16usize;
    let trainer_size = if has_trainer { 512 } else { 0 };
    let prg_start = header_size + trainer_size;
    let prg_end = prg_start + prg_size;
    let chr_start = prg_end;
    let chr_end = chr_start + chr_size;

    if bytes.len() < prg_end {
        return Err(ParseError::TruncatedBody {
            expected: prg_end,
            actual: bytes.len(),
        });
    }
    if chr_size > 0 && bytes.len() < chr_end {
        return Err(ParseError::TruncatedBody {
            expected: chr_end,
            actual: bytes.len(),
        });
    }

    let prg_rom = bytes[prg_start..prg_end].to_vec();
    let use_chr_ram = chr_size == 0;
    let chr = if use_chr_ram {
        // Standard 8 KB CHR-RAM allocation; mappers that need more override this.
        vec![0u8; 8 * 1024]
    } else {
        bytes[chr_start..chr_end].to_vec()
    };

    let sha1 = sha1_hex(bytes);

    let mapper: Box<dyn Mapper> = match mapper_num {
        0 => Box::new(Mapper000::new(
            prg_rom,
            chr,
            mirroring,
            use_chr_ram,
            has_battery,
        )),
        1 => Box::new(Mapper001::new(prg_rom, chr, use_chr_ram, has_battery)),
        2 => Box::new(Mapper002::new(prg_rom, mirroring)),
        3 => Box::new(Mapper003::new(prg_rom, chr, mirroring)),
        4 => Box::new(Mapper004::new(
            prg_rom,
            chr,
            mirroring,
            use_chr_ram,
            has_battery,
        )),
        n => return Err(ParseError::UnsupportedMapper(n)),
    };

    let info = CartridgeInfo {
        mapper: mapper_num,
        prg_rom_size: prg_size,
        chr_rom_size: chr_size,
        mirroring,
        has_battery,
        sha1,
    };

    Ok(Cartridge { info, mapper })
}

/// Compute the SHA-1 of `bytes` and return it as a lowercase hex string.
fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(40);
    for b in digest {
        // Two-digit lowercase hex per byte; writing into a String cannot fail.
        out.push(nibble_to_hex(b >> 4));
        out.push(nibble_to_hex(b & 0x0F));
    }
    out
}

#[inline]
fn nibble_to_hex(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        10..=15 => (b'a' + n - 10) as char,
        _ => '?',
    }
}
