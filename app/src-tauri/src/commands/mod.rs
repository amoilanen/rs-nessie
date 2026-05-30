//! Tauri IPC command handlers.
//!
//! Spec §5.1 lists the complete IPC surface exposed to the frontend. This
//! module groups the handlers by domain — one file per logical command set:
//!
//! - [`library`] — collection CRUD and read-only library listing.
//! - [`rom`] — ROM import / rename / removal (including the native
//!   file-open dialog).
//! - [`emulator`] — start / stop the emulation session and forward live
//!   inputs (button state, pause, volume, mute).
//! - [`settings`] — key-binding and volume persistence.
//! - [`shell`] — small shell-level utilities (fullscreen toggle, open
//!   external URL, log forwarding from the webview).
//!
//! Every handler is a thin `#[tauri::command]` wrapper around a plain Rust
//! helper that takes `&AppState` (and any other plain inputs) and returns
//! `AppResult<T>`. The helper layer exists so the unit tests in this module
//! can exercise the command contract without standing up a full Tauri
//! application.

pub mod emulator;
pub mod library;
pub mod rom;
pub mod settings;
pub mod shell;
