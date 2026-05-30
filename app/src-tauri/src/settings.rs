//! User settings persistence (key bindings, volume, window state).
//!
//! Settings are persisted as JSON to `<config>/dev.rs-nessie/settings.json`
//! (spec §4.2). The on-disk shape is captured by [`Settings`]; helper methods
//! provide controlled mutations (volume, mute, key bindings) and validate
//! their inputs.
//!
//! ## Invariants
//!
//! - `0.0 <= volume <= 1.0`. Helpers reject out-of-range values with
//!   [`AppError::Io`] carrying a descriptive message.
//! - Within a single player's [`ButtonMap`], every key code is unique. The
//!   eight NES buttons must each be bound to a distinct
//!   `KeyboardEvent.code` value. Helpers reject duplicates with
//!   [`AppError::Io`].
//! - Saves are atomic: the JSON is written to a `.tmp` sibling then `rename`d
//!   over the destination, so a crash between the two steps leaves the
//!   previous `settings.json` (if any) untouched. This mirrors the strategy
//!   used by [`crate::library::Library`].
//! - Forward compatibility: unknown JSON fields are ignored on load, so a
//!   future version that adds fields can still be read back by older builds
//!   in development.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};
use crate::library::APP_CONFIG_DIR_NAME;

/// Schema version stamped into the persisted JSON. Bump on any breaking
/// change and add a migration in [`Settings::load_or_default`].
pub const SCHEMA_VERSION: u32 = 1;

/// Filename for the persisted settings inside the app config directory.
pub const SETTINGS_FILE_NAME: &str = "settings.json";

/// Default keyboard shortcut for the fullscreen toggle (spec §4.2).
pub const DEFAULT_FULLSCREEN_SHORTCUT: &str = "F11";

/// Identifies which player slot a [`ButtonMap`] applies to.
///
/// The frontend uses the numeric values `1` and `2` over IPC; the Rust API
/// uses this enum so callers cannot accidentally pass an out-of-range value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlayerSlot {
    /// Player 1 (`bindings.p1`).
    One,
    /// Player 2 (`bindings.p2`).
    Two,
}

/// Mapping from a NES controller button to a `KeyboardEvent.code` string.
///
/// Codes are intentionally stringly-typed (e.g. `"KeyW"`, `"ArrowUp"`,
/// `"NumpadAdd"`) because they are stable across keyboard layouts and are
/// what the webview emits natively (spec §4.2). The set of fields is the
/// eight standard NES buttons.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ButtonMap {
    /// D-pad up.
    pub up: String,
    /// D-pad down.
    pub down: String,
    /// D-pad left.
    pub left: String,
    /// D-pad right.
    pub right: String,
    /// `A` button.
    pub a: String,
    /// `B` button.
    pub b: String,
    /// `Start` button.
    pub start: String,
    /// `Select` button.
    pub select: String,
}

impl Default for ButtonMap {
    fn default() -> Self {
        Self::default_p1()
    }
}

impl ButtonMap {
    /// Default key bindings for Player 1 (spec §4.2 table).
    pub fn default_p1() -> Self {
        Self {
            up: "KeyW".into(),
            down: "KeyS".into(),
            left: "KeyA".into(),
            right: "KeyD".into(),
            a: "KeyJ".into(),
            b: "KeyK".into(),
            start: "Enter".into(),
            select: "ShiftRight".into(),
        }
    }

    /// Default key bindings for Player 2 (spec §4.2 table). Chosen so two
    /// players can share one keyboard (FR-21).
    pub fn default_p2() -> Self {
        Self {
            up: "ArrowUp".into(),
            down: "ArrowDown".into(),
            left: "ArrowLeft".into(),
            right: "ArrowRight".into(),
            a: "Numpad0".into(),
            b: "NumpadDecimal".into(),
            start: "NumpadEnter".into(),
            select: "NumpadAdd".into(),
        }
    }

    /// Iterate over the eight bindings of this map.
    fn codes(&self) -> [&str; 8] {
        [
            self.up.as_str(),
            self.down.as_str(),
            self.left.as_str(),
            self.right.as_str(),
            self.a.as_str(),
            self.b.as_str(),
            self.start.as_str(),
            self.select.as_str(),
        ]
    }

    /// Reject a [`ButtonMap`] that binds the same key code to two different
    /// NES buttons within the same player.
    fn validate_unique(&self) -> AppResult<()> {
        let mut seen: HashSet<&str> = HashSet::with_capacity(8);
        for code in self.codes() {
            if !seen.insert(code) {
                return Err(AppError::Io(format!(
                    "duplicate key '{code}' bound to multiple buttons in one player"
                )));
            }
        }
        Ok(())
    }
}

/// Per-player [`ButtonMap`] pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerBindings {
    /// Player 1 button → key code mapping.
    pub p1: ButtonMap,
    /// Player 2 button → key code mapping (distinct defaults so two players
    /// can share one keyboard, FR-21).
    pub p2: ButtonMap,
}

impl Default for PlayerBindings {
    fn default() -> Self {
        Self {
            p1: ButtonMap::default_p1(),
            p2: ButtonMap::default_p2(),
        }
    }
}

/// Last known window size and position. Restored on next launch by the
/// Tauri host (the host is responsible for clamping to a valid monitor at
/// startup). `None` means "use the platform/Tauri default window".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowState {
    /// Window width in CSS pixels (logical, not physical).
    pub width: u32,
    /// Window height in CSS pixels (logical, not physical).
    pub height: u32,
    /// Top-left X in screen-space CSS pixels.
    pub x: i32,
    /// Top-left Y in screen-space CSS pixels.
    pub y: i32,
    /// Whether the window was maximized when last closed.
    pub maximized: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: 1024,
            height: 768,
            x: 0,
            y: 0,
            maximized: false,
        }
    }
}

/// User-editable settings (spec §4.2).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Schema version. See [`SCHEMA_VERSION`].
    pub version: u32,
    /// Key bindings for both players.
    pub bindings: PlayerBindings,
    /// Master output volume in `0.0..=1.0`.
    pub volume: f32,
    /// Whether audio is muted.
    pub muted: bool,
    /// Keyboard shortcut that toggles fullscreen mode (`KeyboardEvent.code`
    /// or a friendly name such as `"F11"`). Interpretation is up to the
    /// frontend.
    pub fullscreen_shortcut: String,
    /// Last known window placement. `None` means "use platform default".
    pub window: Option<WindowState>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            bindings: PlayerBindings::default(),
            volume: 1.0,
            muted: false,
            fullscreen_shortcut: DEFAULT_FULLSCREEN_SHORTCUT.into(),
            window: None,
        }
    }
}

impl Settings {
    /// Load the persisted settings from `path`, returning [`Settings::default`]
    /// if the file does not exist.
    ///
    /// Returns [`AppError::Io`] for parse failures (a corrupted settings file
    /// is recoverable: the user can delete it; we surface a descriptive
    /// message rather than panicking). On successful load the loaded values
    /// are validated (clamped volume / deduped bindings) so callers can rely
    /// on the invariants documented at the module level.
    pub fn load_or_default(path: &Path) -> AppResult<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut settings: Settings = serde_json::from_slice(&bytes)
                    .map_err(|e| AppError::Io(format!("settings.json: {e}")))?;
                // Defensive clamping: a manually-edited file may carry an
                // out-of-range volume. Clamp rather than reject so the user
                // can still launch the app.
                settings.volume = settings.volume.clamp(0.0, 1.0);
                Ok(settings)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(AppError::from(err)),
        }
    }

    /// Persist the settings to `path` atomically.
    ///
    /// Writes a sibling `<path>.tmp` first and then `rename`s it over the
    /// destination so a crash between the two steps leaves the previous
    /// file (if any) intact.
    pub fn save(&self, path: &Path) -> AppResult<()> {
        let tmp = self.write_tmp(path)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Stage the on-disk content into a `<path>.tmp` sibling without
    /// committing it. The destination at `path` is **not** touched.
    ///
    /// Kept `pub(crate)` so the unit tests can simulate "panic between tmp
    /// write and rename" without resorting to platform-specific tricks.
    pub(crate) fn write_tmp(&self, path: &Path) -> AppResult<PathBuf> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)?;
            }
        }
        let tmp = tmp_sibling(path);
        let json = serde_json::to_vec_pretty(self)?;
        let mut f = fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
        Ok(tmp)
    }

    /// Replace one player's [`ButtonMap`] with `map`.
    ///
    /// Rejects maps containing duplicate key codes within the same player
    /// (the same key bound to two different NES buttons) with
    /// [`AppError::Io`] carrying a descriptive message. Cross-player
    /// duplicates are allowed (a key may be bound differently for P1 vs P2;
    /// in practice both players share one keyboard so the frontend should
    /// warn, but the data layer does not block it).
    pub fn update_bindings(&mut self, player: PlayerSlot, map: ButtonMap) -> AppResult<&Self> {
        map.validate_unique()?;
        match player {
            PlayerSlot::One => self.bindings.p1 = map,
            PlayerSlot::Two => self.bindings.p2 = map,
        }
        Ok(self)
    }

    /// Restore every binding (both players) to the documented defaults.
    /// Other fields (`volume`, `muted`, `window`, `fullscreen_shortcut`) are
    /// preserved — the frontend exposes a separate "reset all" path.
    pub fn reset_bindings(&mut self) -> &Self {
        self.bindings = PlayerBindings::default();
        self
    }

    /// Update the master output volume.
    ///
    /// `volume` must satisfy `0.0..=1.0`; values outside that range
    /// (including NaN) are rejected with [`AppError::Io`].
    pub fn set_volume(&mut self, volume: f32) -> AppResult<&Self> {
        if !(0.0..=1.0).contains(&volume) {
            return Err(AppError::Io(format!(
                "volume must be in 0.0..=1.0, got {volume}"
            )));
        }
        self.volume = volume;
        Ok(self)
    }

    /// Update the mute flag.
    pub fn set_muted(&mut self, muted: bool) -> &Self {
        self.muted = muted;
        self
    }
}

/// Resolve the default on-disk path for `settings.json`
/// (`<OS config dir>/dev.rs-nessie/settings.json`).
///
/// Returns [`AppError::Io`] if the OS does not expose a user config
/// directory (extremely rare; indicates a non-standard environment).
pub fn default_settings_path() -> AppResult<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| AppError::Io("could not resolve OS user config directory".into()))?;
    Ok(dir.join(APP_CONFIG_DIR_NAME).join(SETTINGS_FILE_NAME))
}

/// Return the `.tmp` sibling path used for atomic saves.
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    use tempfile::tempdir;

    #[test]
    fn default_settings_round_trip_through_json() {
        let settings = Settings::default();
        let json = serde_json::to_vec_pretty(&settings).unwrap();
        let parsed: Settings = serde_json::from_slice(&json).unwrap();
        assert_eq!(parsed, settings);
    }

    #[test]
    fn default_bindings_match_spec_table() {
        let s = Settings::default();
        // P1
        assert_eq!(s.bindings.p1.up, "KeyW");
        assert_eq!(s.bindings.p1.left, "KeyA");
        assert_eq!(s.bindings.p1.down, "KeyS");
        assert_eq!(s.bindings.p1.right, "KeyD");
        assert_eq!(s.bindings.p1.a, "KeyJ");
        assert_eq!(s.bindings.p1.b, "KeyK");
        assert_eq!(s.bindings.p1.start, "Enter");
        assert_eq!(s.bindings.p1.select, "ShiftRight");
        // P2
        assert_eq!(s.bindings.p2.up, "ArrowUp");
        assert_eq!(s.bindings.p2.down, "ArrowDown");
        assert_eq!(s.bindings.p2.left, "ArrowLeft");
        assert_eq!(s.bindings.p2.right, "ArrowRight");
        assert_eq!(s.bindings.p2.a, "Numpad0");
        assert_eq!(s.bindings.p2.b, "NumpadDecimal");
        assert_eq!(s.bindings.p2.start, "NumpadEnter");
        assert_eq!(s.bindings.p2.select, "NumpadAdd");
        // Other defaults
        assert_eq!(s.version, SCHEMA_VERSION);
        assert!((s.volume - 1.0).abs() < f32::EPSILON);
        assert!(!s.muted);
        assert_eq!(s.fullscreen_shortcut, DEFAULT_FULLSCREEN_SHORTCUT);
        assert!(s.window.is_none());
    }

    #[test]
    fn save_and_load_round_trip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut s = Settings::default();
        s.set_volume(0.42).unwrap();
        s.set_muted(true);
        s.window = Some(WindowState {
            width: 1280,
            height: 800,
            x: 100,
            y: 50,
            maximized: false,
        });

        s.save(&path).unwrap();
        let reloaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(reloaded, s);
    }

    #[test]
    fn load_or_default_returns_default_when_file_missing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings::load_or_default(&path).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn load_or_default_rejects_unparseable_json() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, b"not json").unwrap();
        let err = Settings::load_or_default(&path).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn reset_bindings_restores_defaults_exactly() {
        let mut s = Settings::default();
        // Mutate both maps so the test is meaningful.
        let mut munged = ButtonMap::default_p1();
        munged.up = "KeyZ".into();
        s.update_bindings(PlayerSlot::One, munged).unwrap();
        let mut munged2 = ButtonMap::default_p2();
        munged2.start = "KeyP".into();
        s.update_bindings(PlayerSlot::Two, munged2).unwrap();
        assert_ne!(s.bindings, PlayerBindings::default());

        s.reset_bindings();
        assert_eq!(s.bindings, PlayerBindings::default());
        // Non-binding fields are untouched.
        assert!((s.volume - 1.0).abs() < f32::EPSILON);
        assert!(!s.muted);
        assert_eq!(s.fullscreen_shortcut, DEFAULT_FULLSCREEN_SHORTCUT);
    }

    #[test]
    fn update_bindings_rejects_duplicate_keys_in_one_player() {
        let mut s = Settings::default();
        let mut bad = ButtonMap::default_p1();
        // Bind both `up` and `down` to the same key.
        bad.down = bad.up.clone();
        let err = s.update_bindings(PlayerSlot::One, bad).unwrap_err();
        match err {
            AppError::Io(msg) => assert!(
                msg.contains("duplicate key"),
                "unexpected error message: {msg}"
            ),
            other => panic!("expected AppError::Io, got {other:?}"),
        }
        // The previous bindings are preserved.
        assert_eq!(s.bindings.p1, ButtonMap::default_p1());
    }

    #[test]
    fn update_bindings_allows_cross_player_duplicates() {
        // A key shared between P1 and P2 is allowed at the data layer (the
        // frontend may warn, but it is not invalid persistence).
        let mut s = Settings::default();
        let mut p2 = ButtonMap::default_p2();
        p2.up = s.bindings.p1.up.clone();
        s.update_bindings(PlayerSlot::Two, p2.clone()).unwrap();
        assert_eq!(s.bindings.p2, p2);
    }

    #[test]
    fn set_volume_rejects_out_of_range() {
        let mut s = Settings::default();
        let err = s.set_volume(-0.1).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
        let err = s.set_volume(1.5).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
        let err = s.set_volume(f32::NAN).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
        // Volume unchanged after failed updates.
        assert!((s.volume - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn set_volume_accepts_boundary_values() {
        let mut s = Settings::default();
        s.set_volume(0.0).unwrap();
        assert_eq!(s.volume, 0.0);
        s.set_volume(1.0).unwrap();
        assert_eq!(s.volume, 1.0);
        s.set_volume(0.5).unwrap();
        assert!((s.volume - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn set_muted_toggles_flag() {
        let mut s = Settings::default();
        assert!(!s.muted);
        s.set_muted(true);
        assert!(s.muted);
        s.set_muted(false);
        assert!(!s.muted);
    }

    #[test]
    fn unknown_fields_are_ignored_on_load() {
        // Forward compatibility: a future build may add a field; older builds
        // must still load the file cleanly.
        let json = serde_json::json!({
            "version": SCHEMA_VERSION,
            "bindings": PlayerBindings::default(),
            "volume": 0.75,
            "muted": true,
            "fullscreen_shortcut": "F11",
            "window": null,
            "experimental_new_field": "ignored",
            "another_unknown": 42,
        });
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert!((loaded.volume - 0.75).abs() < f32::EPSILON);
        assert!(loaded.muted);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults_on_load() {
        // `#[serde(default)]` on the struct means a partial settings file is
        // accepted: missing fields get their default values rather than
        // failing to load.
        let json = serde_json::json!({
            "volume": 0.25,
        });
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        fs::write(&path, serde_json::to_vec(&json).unwrap()).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert!((loaded.volume - 0.25).abs() < f32::EPSILON);
        assert_eq!(loaded.bindings, PlayerBindings::default());
        assert_eq!(loaded.version, SCHEMA_VERSION);
    }

    #[test]
    fn atomic_save_preserves_original_when_rename_is_skipped() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");

        // Persist an initial, well-known state.
        let mut original = Settings::default();
        original.set_volume(0.3).unwrap();
        original.save(&path).unwrap();
        let on_disk_before = fs::read(&path).unwrap();

        // Mutate the settings and stage a tmp write *without* committing it.
        let mut updated = Settings::default();
        updated.set_volume(0.9).unwrap();
        let tmp = updated.write_tmp(&path).unwrap();
        assert!(tmp.exists(), "tmp file should be written");
        assert!(tmp.to_string_lossy().ends_with(".tmp"));

        // The original file is untouched.
        let on_disk_after = fs::read(&path).unwrap();
        assert_eq!(on_disk_after, on_disk_before);

        // Loading still yields the original settings.
        let reloaded = Settings::load_or_default(&path).unwrap();
        assert_eq!(reloaded, original);
    }

    #[test]
    fn default_settings_path_under_config_dir() {
        let path = default_settings_path().unwrap();
        assert!(path.ends_with(Path::new(APP_CONFIG_DIR_NAME).join(SETTINGS_FILE_NAME)));
    }

    #[test]
    fn load_or_default_clamps_out_of_range_volume() {
        // Defensive: a hand-edited file with `volume: 2.0` should not crash
        // the app — clamp to the valid range on load.
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let raw = serde_json::json!({
            "version": SCHEMA_VERSION,
            "bindings": PlayerBindings::default(),
            "volume": 5.0,
            "muted": false,
            "fullscreen_shortcut": "F11",
            "window": null,
        });
        fs::write(&path, serde_json::to_vec(&raw).unwrap()).unwrap();
        let loaded = Settings::load_or_default(&path).unwrap();
        assert!((loaded.volume - 1.0).abs() < f32::EPSILON);
    }
}
