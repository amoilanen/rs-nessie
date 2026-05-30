//! ROM import / rename / removal IPC commands (spec §5.1 — ROM group).

use std::path::{Path, PathBuf};

use tauri::{AppHandle, Runtime, State};
use tauri_plugin_dialog::DialogExt;

use crate::commands::library::persist_library;
use crate::error::{AppError, AppResult};
use crate::library::{RomEntry, RomId};
use crate::state::AppState;

/// `importRomFromPath(path)` — import a ROM that is already on disk
/// at `path`. The library is persisted before the call returns.
#[tauri::command]
pub fn import_rom_from_path(state: State<'_, AppState>, path: String) -> AppResult<RomEntry> {
    import_rom_from_path_impl(&state, path)
}

/// `importRomFromDialog()` — spawn the native file-open dialog and import
/// the user's selection. Returns `None` if the user cancelled.
#[tauri::command]
pub async fn import_rom_from_dialog<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
) -> AppResult<Option<RomEntry>> {
    let picked = app
        .dialog()
        .file()
        .add_filter("NES ROM", &["nes"])
        .blocking_pick_file();
    let Some(file_path) = picked else {
        return Ok(None);
    };
    let path = file_path
        .into_path()
        .map_err(|e| AppError::Io(format!("invalid dialog path: {e}")))?;
    let entry = import_rom_from_path_impl(&state, path.to_string_lossy().into_owned())?;
    Ok(Some(entry))
}

/// `removeRomFromLibrary(id)` — drop the entry and strip its id from every
/// collection.
#[tauri::command]
pub fn remove_rom_from_library(state: State<'_, AppState>, id: RomId) -> AppResult<()> {
    remove_rom_from_library_impl(&state, id)
}

/// `renameRom(id, title)` — set a new display title.
#[tauri::command]
pub fn rename_rom(state: State<'_, AppState>, id: RomId, title: String) -> AppResult<RomEntry> {
    rename_rom_impl(&state, id, title)
}

// ---------------------------------------------------------------------
// Helpers (test-friendly)
// ---------------------------------------------------------------------

pub(crate) fn import_rom_from_path_impl(state: &AppState, path: String) -> AppResult<RomEntry> {
    let path: PathBuf = PathBuf::from(path);
    if !path.exists() {
        return Err(AppError::RomMissing(path));
    }
    let entry = {
        let mut lib = state.library.lock();
        lib.import_rom(&path)?
    };
    persist_library(state)?;
    Ok(entry)
}

/// Used by the emulator command group to validate a "path" ROM source — the
/// path must exist on disk before we hand it to the runtime.
pub(crate) fn ensure_path_exists(path: &Path) -> AppResult<()> {
    if path.exists() {
        Ok(())
    } else {
        Err(AppError::RomMissing(path.to_path_buf()))
    }
}

pub(crate) fn remove_rom_from_library_impl(state: &AppState, id: RomId) -> AppResult<()> {
    {
        let mut lib = state.library.lock();
        lib.remove_rom(id)?;
    }
    persist_library(state)
}

pub(crate) fn rename_rom_impl(state: &AppState, id: RomId, title: String) -> AppResult<RomEntry> {
    let entry = {
        let mut lib = state.library.lock();
        lib.rename_rom(id, title)?
    };
    persist_library(state)?;
    Ok(entry)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn nrom_bytes() -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1);
        rom.push(1);
        rom.push(0);
        rom.push(0);
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        rom
    }

    fn fixture_state() -> (tempfile::TempDir, AppState) {
        let dir = tempdir().unwrap();
        let state = AppState::with_paths(
            dir.path().join("library.json"),
            dir.path().join("settings.json"),
            dir.path().join("saves"),
        );
        (dir, state)
    }

    #[test]
    fn import_rom_from_path_returns_entry_and_persists() {
        let (dir, state) = fixture_state();
        let rom_path = dir.path().join("game.nes");
        fs::write(&rom_path, nrom_bytes()).unwrap();
        let entry =
            import_rom_from_path_impl(&state, rom_path.to_string_lossy().to_string()).unwrap();
        assert_eq!(entry.mapper, 0);
        // Persisted library exists on disk.
        assert!(state.library_path.exists());
        let loaded = crate::library::Library::load_or_default(&state.library_path).unwrap();
        assert_eq!(loaded.file().roms.len(), 1);
        assert_eq!(loaded.file().roms[0].id, entry.id);
    }

    #[test]
    fn import_rom_from_path_invalid_bytes_returns_invalid_rom() {
        let (dir, state) = fixture_state();
        let rom_path = dir.path().join("garbage.nes");
        fs::write(&rom_path, b"not a rom").unwrap();
        let err =
            import_rom_from_path_impl(&state, rom_path.to_string_lossy().to_string()).unwrap_err();
        assert!(matches!(err, AppError::InvalidRom(_)));
    }

    #[test]
    fn import_rom_from_path_missing_file_returns_rom_missing() {
        let (dir, state) = fixture_state();
        let rom_path = dir.path().join("nope.nes");
        let err =
            import_rom_from_path_impl(&state, rom_path.to_string_lossy().to_string()).unwrap_err();
        assert!(matches!(err, AppError::RomMissing(_)));
    }

    #[test]
    fn import_rom_dedupes_same_content_across_calls() {
        let (dir, state) = fixture_state();
        let a = dir.path().join("a.nes");
        let b = dir.path().join("b.nes");
        fs::write(&a, nrom_bytes()).unwrap();
        fs::write(&b, nrom_bytes()).unwrap();

        let first = import_rom_from_path_impl(&state, a.to_string_lossy().to_string()).unwrap();
        let second = import_rom_from_path_impl(&state, b.to_string_lossy().to_string()).unwrap();
        assert_eq!(first.id, second.id);
        let snapshot = state.library.lock().file().clone();
        assert_eq!(snapshot.roms.len(), 1);
    }

    #[test]
    fn rename_rom_updates_title() {
        let (dir, state) = fixture_state();
        let rom_path = dir.path().join("game.nes");
        fs::write(&rom_path, nrom_bytes()).unwrap();
        let entry =
            import_rom_from_path_impl(&state, rom_path.to_string_lossy().to_string()).unwrap();
        let renamed = rename_rom_impl(&state, entry.id, "My Game".into()).unwrap();
        assert_eq!(renamed.title, "My Game");
    }

    #[test]
    fn remove_rom_from_library_returns_not_found_for_unknown_id() {
        let (_dir, state) = fixture_state();
        let err = remove_rom_from_library_impl(&state, uuid::Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }
}
