//! Library / collection IPC commands (spec §5.1 — Library group).
//!
//! Each `#[tauri::command]` is a paper-thin wrapper over a
//! `pub(crate) fn …_impl(state: &AppState, …)` helper. Tests drive the
//! `_impl` functions directly so they do not need a live Tauri application
//! to validate the IPC contract.

use tauri::State;

use crate::error::AppResult;
use crate::library::{Collection, CollectionId, LibraryFile, RomId};
use crate::state::AppState;

/// `listLibrary()` — return the current in-memory library snapshot.
#[tauri::command]
pub fn list_library(state: State<'_, AppState>) -> AppResult<LibraryFile> {
    list_library_impl(&state)
}

/// `createCollection(name)` — create an empty collection (unique name).
#[tauri::command]
pub fn create_collection(state: State<'_, AppState>, name: String) -> AppResult<Collection> {
    create_collection_impl(&state, name)
}

/// `renameCollection(id, name)` — rename a collection.
#[tauri::command]
pub fn rename_collection(
    state: State<'_, AppState>,
    id: CollectionId,
    name: String,
) -> AppResult<Collection> {
    rename_collection_impl(&state, id, name)
}

/// `deleteCollection(id)` — remove a collection (ROMs themselves are
/// retained in the library).
#[tauri::command]
pub fn delete_collection(state: State<'_, AppState>, id: CollectionId) -> AppResult<()> {
    delete_collection_impl(&state, id)
}

/// `addRomToCollection(collection, rom)` — append a ROM to a collection.
#[tauri::command]
pub fn add_rom_to_collection(
    state: State<'_, AppState>,
    collection: CollectionId,
    rom: RomId,
) -> AppResult<()> {
    add_rom_to_collection_impl(&state, collection, rom)
}

/// `removeRomFromCollection(collection, rom)` — drop a ROM from a
/// collection.
#[tauri::command]
pub fn remove_rom_from_collection(
    state: State<'_, AppState>,
    collection: CollectionId,
    rom: RomId,
) -> AppResult<()> {
    remove_rom_from_collection_impl(&state, collection, rom)
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

/// Atomically clone the on-disk-shaped library file for the frontend.
pub(crate) fn list_library_impl(state: &AppState) -> AppResult<LibraryFile> {
    let lib = state.library.lock();
    Ok(lib.file().clone())
}

pub(crate) fn create_collection_impl(state: &AppState, name: String) -> AppResult<Collection> {
    let mut lib = state.library.lock();
    let collection = lib.create_collection(name)?;
    lib.save(&state.library_path)?;
    Ok(collection)
}

pub(crate) fn rename_collection_impl(
    state: &AppState,
    id: CollectionId,
    name: String,
) -> AppResult<Collection> {
    let mut lib = state.library.lock();
    let collection = lib.rename_collection(id, name)?;
    lib.save(&state.library_path)?;
    Ok(collection)
}

pub(crate) fn delete_collection_impl(state: &AppState, id: CollectionId) -> AppResult<()> {
    let mut lib = state.library.lock();
    lib.delete_collection(id)?;
    lib.save(&state.library_path)?;
    Ok(())
}

pub(crate) fn add_rom_to_collection_impl(
    state: &AppState,
    collection: CollectionId,
    rom: RomId,
) -> AppResult<()> {
    let mut lib = state.library.lock();
    lib.add_rom_to_collection(collection, rom)?;
    lib.save(&state.library_path)?;
    Ok(())
}

pub(crate) fn remove_rom_from_collection_impl(
    state: &AppState,
    collection: CollectionId,
    rom: RomId,
) -> AppResult<()> {
    let mut lib = state.library.lock();
    lib.remove_rom_from_collection(collection, rom)?;
    lib.save(&state.library_path)?;
    Ok(())
}

/// Persist the current in-memory library to disk via the configured path.
///
/// Helper used by sibling command modules (e.g. `rom.rs`) so they do not
/// need to learn `state.library_path` themselves.
pub(crate) fn persist_library(state: &AppState) -> AppResult<()> {
    let lib = state.library.lock();
    lib.save(&state.library_path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::commands::rom::import_rom_from_path_impl;
    use crate::error::AppError;

    /// 24 KB NROM (16 KB PRG + 8 KB CHR) used as a stand-in for any valid ROM.
    fn nrom_bytes(prg_fill: u8, chr_fill: u8) -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1);
        rom.push(1);
        rom.push(0);
        rom.push(0);
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(prg_fill).take(16 * 1024));
        rom.extend(std::iter::repeat(chr_fill).take(8 * 1024));
        rom
    }

    fn fixture_state() -> (tempfile::TempDir, AppState, PathBuf) {
        let dir = tempdir().unwrap();
        let library_path = dir.path().join("library.json");
        let settings_path = dir.path().join("settings.json");
        let saves_dir = dir.path().join("saves");
        let state = AppState::with_paths(library_path.clone(), settings_path, saves_dir);
        (dir, state, library_path)
    }

    #[test]
    fn create_then_list_reflects_membership() {
        let (dir, state, _path) = fixture_state();
        // Seed a ROM via the rom command (covers the contract end-to-end).
        let rom_path = dir.path().join("game.nes");
        fs::write(&rom_path, nrom_bytes(0xAA, 0xBB)).unwrap();
        let rom =
            import_rom_from_path_impl(&state, rom_path.to_string_lossy().to_string()).unwrap();

        let collection = create_collection_impl(&state, "Favorites".into()).unwrap();
        add_rom_to_collection_impl(&state, collection.id, rom.id).unwrap();

        let snapshot = list_library_impl(&state).unwrap();
        assert_eq!(snapshot.roms.len(), 1);
        assert_eq!(snapshot.collections.len(), 1);
        assert_eq!(snapshot.collections[0].rom_ids, vec![rom.id]);
    }

    #[test]
    fn create_collection_persists_to_disk() {
        let (_dir, state, path) = fixture_state();
        create_collection_impl(&state, "Persisted".into()).unwrap();
        assert!(path.exists(), "library.json should be written");
        let loaded = crate::library::Library::load_or_default(&path).unwrap();
        assert_eq!(loaded.file().collections.len(), 1);
        assert_eq!(loaded.file().collections[0].name, "Persisted");
    }

    #[test]
    fn duplicate_collection_returns_io_error() {
        let (_dir, state, _path) = fixture_state();
        create_collection_impl(&state, "Dupes".into()).unwrap();
        let err = create_collection_impl(&state, "Dupes".into()).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn rename_then_delete_collection() {
        let (_dir, state, _path) = fixture_state();
        let c = create_collection_impl(&state, "Original".into()).unwrap();
        let renamed = rename_collection_impl(&state, c.id, "New".into()).unwrap();
        assert_eq!(renamed.name, "New");

        delete_collection_impl(&state, c.id).unwrap();
        let snapshot = list_library_impl(&state).unwrap();
        assert!(snapshot.collections.is_empty());
    }

    #[test]
    fn remove_rom_from_collection_is_idempotent() {
        let (dir, state, _path) = fixture_state();
        let rom_path = dir.path().join("game.nes");
        fs::write(&rom_path, nrom_bytes(0x01, 0x02)).unwrap();
        let rom =
            import_rom_from_path_impl(&state, rom_path.to_string_lossy().to_string()).unwrap();
        let c = create_collection_impl(&state, "C".into()).unwrap();
        add_rom_to_collection_impl(&state, c.id, rom.id).unwrap();

        remove_rom_from_collection_impl(&state, c.id, rom.id).unwrap();
        // Second call is also a success (idempotent contract).
        remove_rom_from_collection_impl(&state, c.id, rom.id).unwrap();
        let snapshot = list_library_impl(&state).unwrap();
        assert!(snapshot.collections[0].rom_ids.is_empty());
    }
}
