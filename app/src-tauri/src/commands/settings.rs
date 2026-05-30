//! Settings IPC commands (spec §5.1 — Settings group).

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::error::{AppError, AppResult};
use crate::settings::{ButtonMap, PlayerSlot, Settings};
use crate::state::AppState;

/// Frontend-friendly player slot enum used by `updateBindings`. Mirrors
/// [`PlayerSlot`] but is `Serialize` for cross-IPC use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayerArg {
    /// Player 1.
    P1,
    /// Player 2.
    P2,
}

impl From<PlayerArg> for PlayerSlot {
    fn from(p: PlayerArg) -> Self {
        match p {
            PlayerArg::P1 => PlayerSlot::One,
            PlayerArg::P2 => PlayerSlot::Two,
        }
    }
}

fn slot_from_u8(player: u8) -> AppResult<PlayerSlot> {
    match player {
        1 => Ok(PlayerSlot::One),
        2 => Ok(PlayerSlot::Two),
        other => Err(AppError::Io(format!(
            "invalid player slot {other} (expected 1 or 2)"
        ))),
    }
}

/// `getSettings()` — return the current persisted settings snapshot.
#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> AppResult<Settings> {
    get_settings_impl(&state)
}

/// `updateBindings(player, map)` — replace one player's [`ButtonMap`].
#[tauri::command]
pub fn update_bindings(
    state: State<'_, AppState>,
    player: u8,
    map: ButtonMap,
) -> AppResult<Settings> {
    update_bindings_impl(&state, player, map)
}

/// `resetBindings()` — restore the default key bindings for both players.
#[tauri::command]
pub fn reset_bindings(state: State<'_, AppState>) -> AppResult<Settings> {
    reset_bindings_impl(&state)
}

// ---------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------

pub(crate) fn get_settings_impl(state: &AppState) -> AppResult<Settings> {
    Ok(state.settings.lock().clone())
}

pub(crate) fn update_bindings_impl(
    state: &AppState,
    player: u8,
    map: ButtonMap,
) -> AppResult<Settings> {
    let slot = slot_from_u8(player)?;
    let mut settings = state.settings.lock();
    settings.update_bindings(slot, map)?;
    settings.save(&state.settings_path)?;
    Ok(settings.clone())
}

pub(crate) fn reset_bindings_impl(state: &AppState) -> AppResult<Settings> {
    let mut settings = state.settings.lock();
    settings.reset_bindings();
    settings.save(&state.settings_path)?;
    Ok(settings.clone())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use tempfile::tempdir;

    use super::*;
    use crate::settings::PlayerBindings;

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
    fn get_settings_returns_defaults_for_a_fresh_state() {
        let (_dir, state) = fixture_state();
        let s = get_settings_impl(&state).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn update_bindings_persists_and_returns_updated_settings() {
        let (_dir, state) = fixture_state();
        let mut map = ButtonMap::default_p1();
        map.up = "KeyZ".into();
        let updated = update_bindings_impl(&state, 1, map.clone()).unwrap();
        assert_eq!(updated.bindings.p1, map);
        // Persisted to disk so the next load_or_default sees the change.
        let on_disk = Settings::load_or_default(&state.settings_path).unwrap();
        assert_eq!(on_disk.bindings.p1, map);
    }

    #[test]
    fn update_bindings_rejects_duplicate_keys() {
        let (_dir, state) = fixture_state();
        let mut bad = ButtonMap::default_p1();
        // Bind both `up` and `down` to the same key.
        bad.down = bad.up.clone();
        let err = update_bindings_impl(&state, 1, bad).unwrap_err();
        match err {
            AppError::Io(msg) => assert!(msg.contains("duplicate key"), "got: {msg}"),
            other => panic!("expected Io, got {other:?}"),
        }
        // Previous bindings remain.
        let s = get_settings_impl(&state).unwrap();
        assert_eq!(s.bindings, PlayerBindings::default());
    }

    #[test]
    fn update_bindings_rejects_invalid_player_slot() {
        let (_dir, state) = fixture_state();
        let err = update_bindings_impl(&state, 3, ButtonMap::default_p1()).unwrap_err();
        assert!(matches!(err, AppError::Io(_)));
    }

    #[test]
    fn reset_bindings_restores_defaults() {
        let (_dir, state) = fixture_state();
        let mut munged = ButtonMap::default_p1();
        munged.up = "KeyZ".into();
        update_bindings_impl(&state, 1, munged).unwrap();

        let s = reset_bindings_impl(&state).unwrap();
        assert_eq!(s.bindings, PlayerBindings::default());
        let on_disk = Settings::load_or_default(&state.settings_path).unwrap();
        assert_eq!(on_disk.bindings, PlayerBindings::default());
    }
}
