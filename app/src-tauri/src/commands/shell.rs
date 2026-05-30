//! Shell-level utility IPC commands (spec §5.1 — Shell group).

use std::process::Command;

use serde::{Deserialize, Serialize};
use tauri::{Runtime, WebviewWindow};

use crate::error::{AppError, AppResult};

/// Response of [`toggle_fullscreen`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct FullscreenState {
    /// `true` after the toggle if the window is now fullscreen.
    pub fullscreen: bool,
}

/// `toggleFullscreen()` — flip the main window's fullscreen flag.
#[tauri::command]
pub fn toggle_fullscreen<R: Runtime>(window: WebviewWindow<R>) -> AppResult<FullscreenState> {
    let current = window
        .is_fullscreen()
        .map_err(|e| AppError::Io(format!("is_fullscreen: {e}")))?;
    let next = !current;
    window
        .set_fullscreen(next)
        .map_err(|e| AppError::Io(format!("set_fullscreen: {e}")))?;
    Ok(FullscreenState { fullscreen: next })
}

/// `openExternal(url)` — hand `url` off to the OS default opener.
///
/// We delegate to the platform's default URL handler via `Command` rather
/// than pulling in another dependency. Failures (no such handler, user
/// declined) surface as [`AppError::Io`].
#[tauri::command]
pub fn open_external<R: Runtime>(_app: tauri::AppHandle<R>, url: String) -> AppResult<()> {
    open_external_impl(&url)
}

/// `log(level, message)` — forward a frontend log line into the Rust `log`
/// facade so it appears in `rs-nessie.log` alongside backend events
/// (spec §6.4).
#[tauri::command]
pub fn log(level: String, message: String) -> AppResult<()> {
    log_impl(&level, &message);
    Ok(())
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

pub(crate) fn open_external_impl(url: &str) -> AppResult<()> {
    if url.trim().is_empty() {
        return Err(AppError::Io("empty url".into()));
    }
    // Reject obviously dangerous schemes; the frontend only ever passes
    // `https://` repository / docs URLs.
    let lower = url.trim_start().to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(AppError::Io(format!(
            "refusing to open non-http URL: {url}"
        )));
    }
    #[cfg(target_os = "macos")]
    let result = Command::new("open").arg(url).spawn();
    #[cfg(target_os = "linux")]
    let result = Command::new("xdg-open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = Command::new("cmd").args(["/C", "start", "", url]).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let result: std::io::Result<std::process::Child> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no URL opener for this platform",
    ));
    result
        .map(|_| ())
        .map_err(|e| AppError::Io(format!("failed to open external URL: {e}")))
}

pub(crate) fn log_impl(level: &str, message: &str) {
    match level.to_ascii_lowercase().as_str() {
        "error" => log::error!("[frontend] {message}"),
        "warn" | "warning" => log::warn!("[frontend] {message}"),
        "debug" => log::debug!("[frontend] {message}"),
        "trace" => log::trace!("[frontend] {message}"),
        _ => log::info!("[frontend] {message}"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn open_external_rejects_empty_url() {
        let err = open_external_impl("").unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn open_external_rejects_non_http_scheme() {
        let err = open_external_impl("file:///etc/passwd").unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
        let err = open_external_impl("javascript:alert(1)").unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn log_impl_does_not_panic_for_any_level() {
        // Just exercise the routing; the actual log output is captured by
        // env_logger if configured.
        for lvl in ["error", "warn", "info", "debug", "trace", "unknown"] {
            log_impl(lvl, "test message");
        }
    }
}
