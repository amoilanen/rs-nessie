//! Public error type for `nessie-core`.
//!
//! Hosts that embed `nessie-core` see a single error type, [`CoreError`].
//! The Tauri host layer maps each variant onto its own user-facing
//! [`AppError`](https://docs.rs/) discriminator (`InvalidRom`,
//! `UnsupportedMapper`, `Io`) per spec §5.2.

use thiserror::Error;

use crate::cart::ParseError;

/// Errors surfaced by the public [`crate::Nes`] facade and the cartridge
/// parser.
///
/// `InvalidRom(String)` carries the human-readable detail for diagnostics so
/// hosts can render a toast without doing their own variant-to-string
/// mapping. `UnsupportedMapper(u16)` separately surfaces the mapper number
/// so the UI can render a more actionable message ("mapper N not yet
/// implemented"). `Io(String)` is reserved for host-side I/O failures the
/// core proxies (currently unused inside the core itself).
#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum CoreError {
    /// The supplied bytes were not a valid iNES / NES 2.0 cartridge.
    #[error("invalid ROM: {0}")]
    InvalidRom(String),
    /// The cartridge's mapper number is not implemented by this build.
    #[error("unsupported mapper {0}")]
    UnsupportedMapper(u16),
    /// A host-level I/O failure (file open, save load, etc.).
    #[error("io: {0}")]
    Io(String),
}

impl From<ParseError> for CoreError {
    fn from(err: ParseError) -> Self {
        match err {
            ParseError::UnsupportedMapper(n) => CoreError::UnsupportedMapper(n),
            other => CoreError::InvalidRom(other.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic)]
    use super::*;

    #[test]
    fn parse_error_unsupported_mapper_maps_through() {
        let err: CoreError = ParseError::UnsupportedMapper(42).into();
        assert_eq!(err, CoreError::UnsupportedMapper(42));
    }

    #[test]
    fn parse_error_other_variants_become_invalid_rom() {
        let err: CoreError = ParseError::InvalidMagic.into();
        match err {
            CoreError::InvalidRom(msg) => assert!(msg.contains("magic")),
            other => panic!("expected InvalidRom, got {other:?}"),
        }
    }
}
