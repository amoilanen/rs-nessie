//! `rs-nessie` — Tauri host library.
//!
//! Module layout:
//! - [`error`] — typed [`error::AppError`] envelope for every IPC command.
//! - [`state`] — the shared [`state::AppState`] mounted as `tauri::State<…>`.
//! - [`library`], [`settings`] — on-disk persistence layers.
//! - [`audio`] — cpal-backed [`nessie_runtime::AudioSink`] implementation.
//! - [`session`] — host-owned [`session::EmulatorSession`] driving the
//!   per-game emulation thread and persisting battery saves.
//! - [`commands`] — Tauri IPC command handlers wired into
//!   `tauri::generate_handler!` from [`run`].
//!
//! The binary entry point is [`run`], invoked by `src/main.rs`. It is exposed
//! from the library so a future mobile target (Tauri 2 supports `#[mobile_entry_point]`)
//! can call it without duplicating the bootstrap.

pub mod audio;
pub mod commands;
pub mod error;
pub mod library;
pub mod session;
pub mod settings;
pub mod state;

use crate::state::AppState;

/// Start the Tauri application.
///
/// Initializes logging, registers the dialog and store plugins, mounts the
/// shared [`AppState`], and registers the full IPC command set (spec §5.1).
pub fn run() {
    // `env_logger` is initialized from the `RS_NESSIE_LOG` env var (default `info`).
    let _ =
        env_logger::Builder::from_env(env_logger::Env::new().filter_or("RS_NESSIE_LOG", "info"))
            .try_init();

    let state = match AppState::new() {
        Ok(s) => s,
        Err(err) => {
            log::error!("fatal: failed to build AppState: {err:?}");
            std::process::exit(1);
        }
    };

    if let Err(err) = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            // library
            commands::library::list_library,
            commands::library::create_collection,
            commands::library::rename_collection,
            commands::library::delete_collection,
            commands::library::add_rom_to_collection,
            commands::library::remove_rom_from_collection,
            // rom
            commands::rom::import_rom_from_path,
            commands::rom::import_rom_from_dialog,
            commands::rom::remove_rom_from_library,
            commands::rom::rename_rom,
            // emulator
            commands::emulator::start_session,
            commands::emulator::stop_session,
            commands::emulator::set_button_state,
            commands::emulator::set_paused,
            commands::emulator::set_muted,
            commands::emulator::set_volume,
            // settings
            commands::settings::get_settings,
            commands::settings::update_bindings,
            commands::settings::reset_bindings,
            // shell
            commands::shell::toggle_fullscreen,
            commands::shell::open_external,
            commands::shell::log,
        ])
        .run(tauri::generate_context!())
    {
        log::error!("fatal: failed to run tauri application: {err}");
        std::process::exit(1);
    }
}
