//! ROM library and collection management with atomic JSON persistence.
//!
//! The [`Library`] owns the in-memory representation of the user's ROM
//! collection plus user-defined [`Collection`]s. It is persisted to a single
//! JSON file (`library.json`) under the OS app-config directory
//! (`<config>/dev.rs-nessie/library.json`, see spec §4.1).
//!
//! ## Invariants
//!
//! - Every [`RomEntry`] in the library has a unique [`RomId`].
//! - Every [`Collection`] has a unique [`CollectionId`] and a unique `name`.
//! - A [`Collection::rom_ids`] entry must refer to a ROM that exists in
//!   [`LibraryFile::roms`]. Dangling references are pruned defensively on
//!   [`Library::load_or_default`] so a manually-edited file cannot crash the
//!   host.
//! - Saves are atomic: the file is first written to a `.tmp` sibling and then
//!   `rename`d over the destination. A crash between the two steps leaves the
//!   previous `library.json` untouched.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use nessie_core::cart::{parse_ines, ParseError};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// Schema version stamped into the persisted JSON. Bump on any breaking change
/// and add a migration in [`Library::load_or_default`].
pub const SCHEMA_VERSION: u32 = 1;

/// Sub-directory under the OS config dir owned by this app.
pub const APP_CONFIG_DIR_NAME: &str = "dev.rs-nessie";

/// Filename for the persisted library inside the app config directory.
pub const LIBRARY_FILE_NAME: &str = "library.json";

/// Stable identifier for a ROM. Generated as a v4 [`Uuid`] on import and
/// never reused.
pub type RomId = Uuid;

/// Stable identifier for a [`Collection`]. Generated as a v4 [`Uuid`] when the
/// collection is created.
pub type CollectionId = Uuid;

/// A single ROM as it appears in the user's library.
///
/// The combination of [`RomEntry::id`] (stable, internal) and
/// [`RomEntry::sha1`] (content hash) lets the host find a ROM's battery save
/// even after the underlying file on disk has been moved or renamed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RomEntry {
    /// Unique, persistent identifier (UUID v4).
    pub id: RomId,
    /// User-editable display title. Defaults to the file stem on import
    /// (FR-10).
    pub title: String,
    /// Absolute path to the ROM on disk. Updated on re-import of the same
    /// content from a new location (FR-14).
    pub path: PathBuf,
    /// SHA-1 of the on-disk ROM bytes (lowercase hex, 40 chars). Used as the
    /// dedup key on import and the save-file key (`saves/<sha1>.srm`).
    pub sha1: String,
    /// iNES mapper number, surfaced for diagnostics in the library UI.
    pub mapper: u16,
    /// Length of the ROM in bytes (post-header, includes payload).
    pub size_bytes: u64,
    /// Unix milliseconds at which the ROM was first imported.
    pub imported_at: i64,
}

/// A user-defined grouping of ROMs.
///
/// A given ROM may appear in any number of collections (many-to-many,
/// FR-11). Collections survive renames and re-imports because they reference
/// [`RomId`]s, not paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Collection {
    /// Unique, persistent identifier (UUID v4).
    pub id: CollectionId,
    /// Human-readable name. Unique across the library (case-sensitive).
    pub name: String,
    /// Ordered list of ROMs that belong to this collection.
    pub rom_ids: Vec<RomId>,
    /// Unix milliseconds at which the collection was created.
    pub created_at: i64,
}

/// Serialization shape for the on-disk `library.json` file.
///
/// Wraps a schema version so future changes can migrate older snapshots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryFile {
    /// Schema version. See [`SCHEMA_VERSION`].
    pub version: u32,
    /// All ROMs known to the user (uniquely identified by [`RomEntry::id`]).
    pub roms: Vec<RomEntry>,
    /// User-defined collections of ROMs.
    pub collections: Vec<Collection>,
}

impl Default for LibraryFile {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            roms: Vec::new(),
            collections: Vec::new(),
        }
    }
}

/// In-memory ROM library plus user-defined collections.
///
/// All mutating operations leave the in-memory state consistent (no dangling
/// references). Persistence is opt-in: call [`Library::save`] to write the
/// current state to disk.
#[derive(Debug, Default)]
pub struct Library {
    file: LibraryFile,
}

impl Library {
    /// Construct a [`Library`] wrapping an explicit [`LibraryFile`].
    ///
    /// Mainly useful for tests and for the round-trip helpers; production code
    /// should use [`Library::load_or_default`].
    pub fn from_file(file: LibraryFile) -> Self {
        Self { file }
    }

    /// Read-only access to the underlying serialization shape (for IPC and
    /// tests).
    pub fn file(&self) -> &LibraryFile {
        &self.file
    }

    /// Look up a ROM by id.
    pub fn rom(&self, id: RomId) -> Option<&RomEntry> {
        self.file.roms.iter().find(|r| r.id == id)
    }

    /// Look up a collection by id.
    pub fn collection(&self, id: CollectionId) -> Option<&Collection> {
        self.file.collections.iter().find(|c| c.id == id)
    }

    /// Load the persisted library from `path`, returning a default empty
    /// library if the file does not exist.
    ///
    /// Defensively prunes any dangling [`Collection::rom_ids`] entries
    /// referencing ROMs no longer present in [`LibraryFile::roms`], so a
    /// hand-edited file never causes the host to panic.
    pub fn load_or_default(path: &Path) -> AppResult<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut file: LibraryFile = serde_json::from_slice(&bytes)
                    .map_err(|e| AppError::LibraryCorrupted(e.to_string()))?;
                prune_dangling(&mut file);
                Ok(Self { file })
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(AppError::from(err)),
        }
    }

    /// Persist the library to `path` atomically.
    ///
    /// Writes a sibling `<path>.tmp` first and then `rename`s it over the
    /// destination so a crash between the two steps leaves the previous file
    /// (if any) intact.
    pub fn save(&self, path: &Path) -> AppResult<()> {
        let tmp = self.write_tmp(path)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Stage the on-disk content into a `<path>.tmp` sibling without
    /// committing it. The destination at `path` is **not** touched.
    ///
    /// Public callers use [`Library::save`] instead. This helper is kept
    /// `pub(crate)` so the unit tests can simulate "panic between tmp write
    /// and rename" without resorting to platform-specific tricks.
    pub(crate) fn write_tmp(&self, path: &Path) -> AppResult<PathBuf> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let tmp = tmp_sibling(path);
        let json = serde_json::to_vec_pretty(&self.file)?;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        Ok(tmp)
    }

    /// Import the ROM at `rom_path` into the library.
    ///
    /// Reads the file, validates it via [`parse_ines`], and either inserts a
    /// new [`RomEntry`] or — if a ROM with the same SHA-1 already exists —
    /// updates that entry's [`RomEntry::path`] and returns it unchanged
    /// otherwise. Unknown mapper numbers surface as
    /// [`AppError::UnsupportedMapper`]; every other parse failure becomes
    /// [`AppError::InvalidRom`] (FR-8).
    pub fn import_rom(&mut self, rom_path: &Path) -> AppResult<RomEntry> {
        let bytes = fs::read(rom_path)?;
        let cart = parse_ines(&bytes).map_err(map_parse_error)?;
        let info = cart.info();
        let sha1 = info.sha1.clone();
        let mapper = info.mapper;
        let size_bytes = bytes.len() as u64;
        let abs_path = rom_path.to_path_buf();

        if let Some(existing) = self.file.roms.iter_mut().find(|r| r.sha1 == sha1) {
            existing.path = abs_path;
            return Ok(existing.clone());
        }

        let title = rom_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| "Untitled".to_owned());
        let entry = RomEntry {
            id: Uuid::new_v4(),
            title,
            path: abs_path,
            sha1,
            mapper,
            size_bytes,
            imported_at: now_ms(),
        };
        self.file.roms.push(entry.clone());
        Ok(entry)
    }

    /// Remove a ROM and strip its id from every collection.
    pub fn remove_rom(&mut self, id: RomId) -> AppResult<()> {
        let initial_len = self.file.roms.len();
        self.file.roms.retain(|r| r.id != id);
        if self.file.roms.len() == initial_len {
            return Err(AppError::NotFound);
        }
        for c in &mut self.file.collections {
            c.rom_ids.retain(|r| *r != id);
        }
        Ok(())
    }

    /// Rename a ROM's display title.
    pub fn rename_rom(&mut self, id: RomId, title: String) -> AppResult<RomEntry> {
        let rom = self
            .file
            .roms
            .iter_mut()
            .find(|r| r.id == id)
            .ok_or(AppError::NotFound)?;
        rom.title = title;
        Ok(rom.clone())
    }

    /// Create a new, empty collection with the given (unique) display name.
    pub fn create_collection(&mut self, name: String) -> AppResult<Collection> {
        if self.file.collections.iter().any(|c| c.name == name) {
            return Err(AppError::Io(format!(
                "a collection named '{name}' already exists"
            )));
        }
        let collection = Collection {
            id: Uuid::new_v4(),
            name,
            rom_ids: Vec::new(),
            created_at: now_ms(),
        };
        self.file.collections.push(collection.clone());
        Ok(collection)
    }

    /// Rename an existing collection. The new name must be unique among all
    /// other collections (case-sensitive).
    pub fn rename_collection(&mut self, id: CollectionId, name: String) -> AppResult<Collection> {
        if self
            .file
            .collections
            .iter()
            .any(|c| c.id != id && c.name == name)
        {
            return Err(AppError::Io(format!(
                "a collection named '{name}' already exists"
            )));
        }
        let collection = self
            .file
            .collections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or(AppError::NotFound)?;
        collection.name = name;
        Ok(collection.clone())
    }

    /// Delete a collection. ROMs in it are not removed from the library; only
    /// the collection's membership record is dropped.
    pub fn delete_collection(&mut self, id: CollectionId) -> AppResult<()> {
        let initial = self.file.collections.len();
        self.file.collections.retain(|c| c.id != id);
        if self.file.collections.len() == initial {
            return Err(AppError::NotFound);
        }
        Ok(())
    }

    /// Append `rom` to `collection`. The operation is idempotent: adding a
    /// ROM that already belongs to the collection is a no-op.
    pub fn add_rom_to_collection(&mut self, collection: CollectionId, rom: RomId) -> AppResult<()> {
        if !self.file.roms.iter().any(|r| r.id == rom) {
            return Err(AppError::NotFound);
        }
        let c = self
            .file
            .collections
            .iter_mut()
            .find(|c| c.id == collection)
            .ok_or(AppError::NotFound)?;
        if !c.rom_ids.contains(&rom) {
            c.rom_ids.push(rom);
        }
        Ok(())
    }

    /// Remove `rom` from `collection`. The operation is idempotent: removing
    /// a ROM not in the collection is a no-op.
    pub fn remove_rom_from_collection(
        &mut self,
        collection: CollectionId,
        rom: RomId,
    ) -> AppResult<()> {
        let c = self
            .file
            .collections
            .iter_mut()
            .find(|c| c.id == collection)
            .ok_or(AppError::NotFound)?;
        c.rom_ids.retain(|r| *r != rom);
        Ok(())
    }
}

/// Resolve the default on-disk path for `library.json`
/// (`<OS config dir>/dev.rs-nessie/library.json`).
///
/// Returns [`AppError::Io`] if the OS does not expose a user config directory
/// (extremely rare; would indicate a non-standard environment).
pub fn default_library_path() -> AppResult<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| AppError::Io("could not resolve OS user config directory".into()))?;
    Ok(dir.join(APP_CONFIG_DIR_NAME).join(LIBRARY_FILE_NAME))
}

/// Return the `.tmp` sibling path used for atomic saves.
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

/// Map `parse_ines`'s error variants onto the host's [`AppError`].
fn map_parse_error(err: ParseError) -> AppError {
    match err {
        ParseError::UnsupportedMapper(n) => AppError::UnsupportedMapper(n),
        other => AppError::InvalidRom(other.to_string()),
    }
}

/// Current unix-epoch millisecond timestamp, saturating at 0 for clocks
/// reporting a pre-epoch time (e.g. a misconfigured embedded device).
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Strip any [`Collection::rom_ids`] entries that no longer refer to a
/// present [`RomEntry`].
fn prune_dangling(file: &mut LibraryFile) {
    let known: HashSet<RomId> = file.roms.iter().map(|r| r.id).collect();
    for c in &mut file.collections {
        c.rom_ids.retain(|id| known.contains(id));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    use tempfile::tempdir;

    /// Build a minimal valid 24 KB NROM ROM (16 KB PRG + 8 KB CHR).
    fn nrom_bytes(prg_fill: u8, chr_fill: u8) -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1); // 16 KB PRG
        rom.push(1); // 8 KB CHR
        rom.push(0); // flags6: horizontal mirroring, no battery
        rom.push(0); // flags7
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(prg_fill).take(16 * 1024));
        rom.extend(std::iter::repeat(chr_fill).take(8 * 1024));
        rom
    }

    /// Write `bytes` to a fresh temp file with extension `.nes`.
    fn write_temp_rom(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn library_file_round_trip_through_json() {
        let mut file = LibraryFile::default();
        let rom = RomEntry {
            id: Uuid::new_v4(),
            title: "Super Demo".into(),
            path: PathBuf::from("/roms/super.nes"),
            sha1: "0".repeat(40),
            mapper: 0,
            size_bytes: 24 * 1024,
            imported_at: 1_700_000_000_000,
        };
        let collection = Collection {
            id: Uuid::new_v4(),
            name: "Favorites".into(),
            rom_ids: vec![rom.id],
            created_at: 1_700_000_001_000,
        };
        file.roms.push(rom);
        file.collections.push(collection);

        let json = serde_json::to_vec_pretty(&file).unwrap();
        let parsed: LibraryFile = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed, file);
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");

        let mut lib = Library::default();
        // Seed with one synthetic ROM and one collection.
        let entry = RomEntry {
            id: Uuid::new_v4(),
            title: "Demo".into(),
            path: PathBuf::from("/roms/demo.nes"),
            sha1: "a".repeat(40),
            mapper: 0,
            size_bytes: 1234,
            imported_at: 42,
        };
        lib.file.roms.push(entry.clone());
        let coll = lib.create_collection("Bench".into()).unwrap();
        lib.add_rom_to_collection(coll.id, entry.id).unwrap();

        lib.save(&path).unwrap();
        let reloaded = Library::load_or_default(&path).unwrap();
        assert_eq!(reloaded.file(), lib.file());
    }

    #[test]
    fn load_or_default_returns_empty_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        let lib = Library::load_or_default(&path).unwrap();
        assert!(lib.file().roms.is_empty());
        assert!(lib.file().collections.is_empty());
        assert_eq!(lib.file().version, SCHEMA_VERSION);
    }

    #[test]
    fn load_or_default_returns_corrupted_for_bad_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");
        fs::write(&path, b"not json").unwrap();
        let err = Library::load_or_default(&path).unwrap_err();
        assert!(matches!(err, AppError::LibraryCorrupted(_)));
    }

    #[test]
    fn import_rom_dedupes_same_content() {
        let dir = tempdir().unwrap();
        let rom_bytes = nrom_bytes(0xAB, 0xCD);
        let a = write_temp_rom(dir.path(), "alpha.nes", &rom_bytes);
        let b = write_temp_rom(dir.path(), "beta.nes", &rom_bytes);

        let mut lib = Library::default();
        let first = lib.import_rom(&a).unwrap();
        let second = lib.import_rom(&b).unwrap();

        // Only one entry exists, but its `path` was updated to the latest
        // import location, and the id is stable.
        assert_eq!(lib.file().roms.len(), 1);
        assert_eq!(first.id, second.id);
        assert_eq!(first.sha1, second.sha1);
        assert_eq!(second.path, b);
        // Title defaults to the first file's stem on initial import.
        assert_eq!(first.title, "alpha");
        assert_eq!(second.title, "alpha");
    }

    #[test]
    fn import_rom_rejects_invalid_bytes() {
        let dir = tempdir().unwrap();
        let path = write_temp_rom(dir.path(), "garbage.nes", b"this is not a ROM");
        let mut lib = Library::default();
        let err = lib.import_rom(&path).unwrap_err();
        assert!(matches!(err, AppError::InvalidRom(_)));
    }

    #[test]
    fn import_rom_rejects_unsupported_mapper() {
        // Build a ROM whose mapper number is 5 (MMC5, unsupported).
        let dir = tempdir().unwrap();
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1);
        rom.push(1);
        rom.push(0x50); // mapper number 5 in the high nibble of flags6
        rom.push(0x00);
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        let path = write_temp_rom(dir.path(), "mmc5.nes", &rom);

        let mut lib = Library::default();
        let err = lib.import_rom(&path).unwrap_err();
        assert!(matches!(err, AppError::UnsupportedMapper(5)));
    }

    #[test]
    fn import_rom_distinct_content_creates_two_entries() {
        let dir = tempdir().unwrap();
        let a_path = write_temp_rom(dir.path(), "a.nes", &nrom_bytes(0x11, 0x22));
        let b_path = write_temp_rom(dir.path(), "b.nes", &nrom_bytes(0x33, 0x44));

        let mut lib = Library::default();
        let a = lib.import_rom(&a_path).unwrap();
        let b = lib.import_rom(&b_path).unwrap();
        assert_ne!(a.id, b.id);
        assert_ne!(a.sha1, b.sha1);
        assert_eq!(lib.file().roms.len(), 2);
    }

    #[test]
    fn load_or_default_prunes_dangling_collection_refs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");

        let real_id = Uuid::new_v4();
        let ghost_id = Uuid::new_v4();
        let file = LibraryFile {
            version: SCHEMA_VERSION,
            roms: vec![RomEntry {
                id: real_id,
                title: "Real".into(),
                path: PathBuf::from("/roms/real.nes"),
                sha1: "f".repeat(40),
                mapper: 0,
                size_bytes: 0,
                imported_at: 0,
            }],
            collections: vec![Collection {
                id: Uuid::new_v4(),
                name: "Mixed".into(),
                rom_ids: vec![real_id, ghost_id],
                created_at: 0,
            }],
        };
        fs::write(&path, serde_json::to_vec(&file).unwrap()).unwrap();

        let lib = Library::load_or_default(&path).unwrap();
        let coll = &lib.file().collections[0];
        assert_eq!(coll.rom_ids, vec![real_id]);
    }

    #[test]
    fn remove_rom_strips_membership_from_every_collection() {
        let mut lib = Library::default();
        let dir = tempdir().unwrap();
        let path = write_temp_rom(dir.path(), "rom.nes", &nrom_bytes(0x01, 0x02));
        let rom = lib.import_rom(&path).unwrap();

        let c1 = lib.create_collection("One".into()).unwrap();
        let c2 = lib.create_collection("Two".into()).unwrap();
        lib.add_rom_to_collection(c1.id, rom.id).unwrap();
        lib.add_rom_to_collection(c2.id, rom.id).unwrap();

        lib.remove_rom(rom.id).unwrap();

        assert!(lib.rom(rom.id).is_none());
        for c in &lib.file().collections {
            assert!(
                !c.rom_ids.contains(&rom.id),
                "ROM not stripped from {}",
                c.name
            );
        }
    }

    #[test]
    fn remove_rom_returns_not_found_for_unknown_id() {
        let mut lib = Library::default();
        let err = lib.remove_rom(Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }

    #[test]
    fn rename_rom_updates_title() {
        let mut lib = Library::default();
        let dir = tempdir().unwrap();
        let path = write_temp_rom(dir.path(), "rom.nes", &nrom_bytes(0x01, 0x02));
        let rom = lib.import_rom(&path).unwrap();
        let renamed = lib.rename_rom(rom.id, "My Game".into()).unwrap();
        assert_eq!(renamed.title, "My Game");
        assert_eq!(lib.rom(rom.id).unwrap().title, "My Game");
    }

    #[test]
    fn create_collection_rejects_duplicate_name() {
        let mut lib = Library::default();
        lib.create_collection("Favorites".into()).unwrap();
        let err = lib.create_collection("Favorites".into()).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn rename_collection_rejects_duplicate_name() {
        let mut lib = Library::default();
        let _ = lib.create_collection("A".into()).unwrap();
        let b = lib.create_collection("B".into()).unwrap();
        let err = lib.rename_collection(b.id, "A".into()).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn delete_collection_removes_it() {
        let mut lib = Library::default();
        let c = lib.create_collection("Temp".into()).unwrap();
        lib.delete_collection(c.id).unwrap();
        assert!(lib.collection(c.id).is_none());
    }

    #[test]
    fn add_rom_to_collection_is_idempotent_and_validated() {
        let mut lib = Library::default();
        let dir = tempdir().unwrap();
        let path = write_temp_rom(dir.path(), "rom.nes", &nrom_bytes(0x01, 0x02));
        let rom = lib.import_rom(&path).unwrap();
        let c = lib.create_collection("X".into()).unwrap();

        lib.add_rom_to_collection(c.id, rom.id).unwrap();
        lib.add_rom_to_collection(c.id, rom.id).unwrap(); // idempotent
        assert_eq!(lib.collection(c.id).unwrap().rom_ids, vec![rom.id]);

        // Unknown ROM id is rejected.
        let err = lib.add_rom_to_collection(c.id, Uuid::new_v4()).unwrap_err();
        assert!(matches!(err, AppError::NotFound));

        // Unknown collection id is rejected.
        let err = lib
            .add_rom_to_collection(Uuid::new_v4(), rom.id)
            .unwrap_err();
        assert!(matches!(err, AppError::NotFound));
    }

    #[test]
    fn remove_rom_from_collection_is_idempotent() {
        let mut lib = Library::default();
        let dir = tempdir().unwrap();
        let path = write_temp_rom(dir.path(), "rom.nes", &nrom_bytes(0x01, 0x02));
        let rom = lib.import_rom(&path).unwrap();
        let c = lib.create_collection("X".into()).unwrap();
        lib.add_rom_to_collection(c.id, rom.id).unwrap();

        lib.remove_rom_from_collection(c.id, rom.id).unwrap();
        // Calling it again is a no-op, not an error.
        lib.remove_rom_from_collection(c.id, rom.id).unwrap();
        assert!(lib.collection(c.id).unwrap().rom_ids.is_empty());
    }

    #[test]
    fn atomic_save_preserves_original_when_rename_is_skipped() {
        // This test models the scenario "we wrote the tmp file but a panic
        // or crash prevented the rename": the previously-persisted library
        // must be returned unchanged on the next load.
        let dir = tempdir().unwrap();
        let path = dir.path().join("library.json");

        // Persist an initial, well-known state.
        let mut original = Library::default();
        let entry = RomEntry {
            id: Uuid::new_v4(),
            title: "Original".into(),
            path: PathBuf::from("/roms/orig.nes"),
            sha1: "c".repeat(40),
            mapper: 0,
            size_bytes: 0,
            imported_at: 7,
        };
        original.file.roms.push(entry.clone());
        original.save(&path).unwrap();
        let on_disk_before = fs::read(&path).unwrap();

        // Mutate the library and stage a tmp write *without* committing it.
        let mut updated = Library::default();
        updated.file.roms.push(RomEntry {
            id: Uuid::new_v4(),
            title: "Updated".into(),
            ..entry
        });
        let tmp = updated.write_tmp(&path).unwrap();
        assert!(tmp.exists(), "tmp file should be written");
        assert!(tmp.to_string_lossy().ends_with(".tmp"));

        // The original file is untouched.
        let on_disk_after = fs::read(&path).unwrap();
        assert_eq!(on_disk_after, on_disk_before);

        // Loading still yields the original library.
        let reloaded = Library::load_or_default(&path).unwrap();
        assert_eq!(reloaded.file(), original.file());
    }

    #[test]
    fn default_library_path_under_config_dir() {
        let path = default_library_path().unwrap();
        assert!(path.ends_with(Path::new(APP_CONFIG_DIR_NAME).join(LIBRARY_FILE_NAME)));
    }
}
