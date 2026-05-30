//! Shared Tauri application state.
//!
//! Held inside the Tauri runtime as `tauri::State<AppState>` and accessed by
//! every IPC command. The individual mutexes are intentionally fine-grained
//! so a long-running emulator operation does not block library or settings
//! reads.

use std::path::PathBuf;

use parking_lot::Mutex;

use crate::error::AppResult;
use crate::library::{default_library_path, Library};
use crate::session::{default_saves_dir, EmulatorSession};
use crate::settings::{default_settings_path, Settings};

/// Root container for everything the Tauri host owns at runtime.
///
/// Each field is wrapped in its own [`parking_lot::Mutex`] so unrelated
/// subsystems (library browsing, settings edits, the active emulator session)
/// do not contend on a single global lock.
#[derive(Debug)]
pub struct AppState {
    /// The user's ROM library and collections.
    pub library: Mutex<Library>,
    /// User-editable settings (key bindings, volume, …).
    pub settings: Mutex<Settings>,
    /// The currently-running emulator session, if any.
    pub session: Mutex<Option<EmulatorSession>>,
    /// On-disk path to the persisted `library.json`.
    pub library_path: PathBuf,
    /// On-disk path to the persisted `settings.json`.
    pub settings_path: PathBuf,
    /// Directory that holds per-cartridge `.srm` battery saves.
    pub saves_dir: PathBuf,
}

impl AppState {
    /// Build a fresh [`AppState`] using the default OS-derived paths
    /// (`<config>/dev.rs-nessie/library.json`, etc.) and load the persisted
    /// library and settings if present.
    ///
    /// Returns [`crate::error::AppError`] only if the OS user-config
    /// directory cannot be resolved or the persisted JSON is corrupt; the
    /// caller (the bin entry point) treats either as a fatal startup error.
    pub fn new() -> AppResult<Self> {
        let library_path = default_library_path()?;
        let settings_path = default_settings_path()?;
        let saves_dir = default_saves_dir()?;
        let library = Library::load_or_default(&library_path)?;
        let settings = Settings::load_or_default(&settings_path)?;
        Ok(Self {
            library: Mutex::new(library),
            settings: Mutex::new(settings),
            session: Mutex::new(None),
            library_path,
            settings_path,
            saves_dir,
        })
    }

    /// Build an [`AppState`] with caller-supplied paths.
    ///
    /// Used by unit tests so they can point everything at a `tempdir` and by
    /// future integration tests / staging hosts that want to override the
    /// default locations.
    pub fn with_paths(library_path: PathBuf, settings_path: PathBuf, saves_dir: PathBuf) -> Self {
        Self {
            library: Mutex::new(Library::default()),
            settings: Mutex::new(Settings::default()),
            session: Mutex::new(None),
            library_path,
            settings_path,
            saves_dir,
        }
    }
}
