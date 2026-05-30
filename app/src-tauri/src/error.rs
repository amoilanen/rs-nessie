//! Typed application error envelope shared between the Rust host and the
//! TypeScript frontend.
//!
//! Every Tauri IPC command returns `Result<T, AppError>`. `AppError` serializes
//! into a discriminated-union JSON shape (`{ "code": "...", "details": ... }`)
//! so the frontend can switch on `code` and render localized toast messages.

use std::path::PathBuf;

use serde::Serialize;
use thiserror::Error;

/// All user-facing errors surfaced through Tauri IPC.
///
/// The `#[serde(tag = "code", content = "details")]` attribute guarantees the
/// frozen JSON shape consumed by `./app/src/ipc/types.ts`. See the unit tests
/// below for the exact wire format snapshots.
#[derive(Debug, Error, Serialize)]
#[serde(tag = "code", content = "details")]
pub enum AppError {
    /// The supplied bytes are not a valid iNES / NES 2.0 ROM.
    #[error("invalid ROM: {0}")]
    InvalidRom(String),

    /// The ROM uses a mapper number the core does not yet implement.
    #[error("unsupported mapper {0}")]
    UnsupportedMapper(u16),

    /// A library entry references a path that no longer exists on disk.
    #[error("ROM not found on disk: {0}")]
    RomMissing(PathBuf),

    /// The persisted library JSON could not be parsed.
    #[error("library corrupted: {0}")]
    LibraryCorrupted(String),

    /// A requested entity (ROM id, collection id, …) does not exist.
    #[error("not found")]
    NotFound,

    /// Generic IO failure (filesystem, audio device open, etc.).
    #[error("io: {0}")]
    Io(String),
}

/// Convenience alias used throughout the host crate.
pub type AppResult<T> = Result<T, AppError>;

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        AppError::Io(err.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::Io(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    /// Frozen JSON shape: `InvalidRom` carries its message under `details`.
    #[test]
    fn invalid_rom_serializes_to_frozen_shape() {
        let err = AppError::InvalidRom("missing NES magic".into());
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(
            json,
            r#"{"code":"InvalidRom","details":"missing NES magic"}"#
        );
    }

    #[test]
    fn unsupported_mapper_serializes_to_frozen_shape() {
        let err = AppError::UnsupportedMapper(9);
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(json, r#"{"code":"UnsupportedMapper","details":9}"#);
    }

    #[test]
    fn rom_missing_serializes_to_frozen_shape() {
        let err = AppError::RomMissing(PathBuf::from("/tmp/missing.nes"));
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(
            json,
            r#"{"code":"RomMissing","details":"/tmp/missing.nes"}"#
        );
    }

    #[test]
    fn library_corrupted_serializes_to_frozen_shape() {
        let err = AppError::LibraryCorrupted("expected ident at line 3".into());
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(
            json,
            r#"{"code":"LibraryCorrupted","details":"expected ident at line 3"}"#
        );
    }

    #[test]
    fn not_found_serializes_without_details() {
        let err = AppError::NotFound;
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(json, r#"{"code":"NotFound"}"#);
    }

    #[test]
    fn io_serializes_to_frozen_shape() {
        let err = AppError::Io("disk full".into());
        let json = serde_json::to_string(&err).expect("serialize");
        assert_eq!(json, r#"{"code":"Io","details":"disk full"}"#);
    }

    #[test]
    fn io_error_conversion_preserves_message() {
        let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let app: AppError = io.into();
        let json = serde_json::to_string(&app).expect("serialize");
        assert_eq!(json, r#"{"code":"Io","details":"denied"}"#);
    }
}
