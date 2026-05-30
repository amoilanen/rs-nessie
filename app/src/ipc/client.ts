// Typed wrappers over the Tauri IPC `invoke` primitive (spec §5.1).
//
// Every command from the Rust host crate is exposed here as a strongly-typed
// async function. The mapping between the JavaScript wrapper name (camelCase)
// and the Tauri command name (snake_case) follows the registration in
// `./app/src-tauri/src/lib.rs::run` via `tauri::generate_handler![...]`.
//
// Errors thrown by `invoke` are inspected: if they look like the wire envelope
// emitted by `AppError` (a `{ code, details? }` object — see spec §5.2 and
// `./app/src-tauri/src/error.rs`), they are rethrown unchanged so callers can
// `switch` on the discriminator. Any other rejection (network, transport,
// runtime error) is also rethrown verbatim — wrappers never swallow errors.

import { Channel, invoke } from '@tauri-apps/api/core';

import type {
  AppError,
  ButtonMap,
  Collection,
  CollectionId,
  FrameMessage,
  FullscreenState,
  LibraryFile,
  NesButton,
  Player,
  RomEntry,
  RomId,
  RomSource,
  SessionInfo,
  Settings,
} from './types';
import { isAppError } from './types';

/**
 * Internal helper: call `invoke` and translate the rejection into a typed
 * [`AppError`] when the wire envelope matches.
 *
 * The shape is preserved exactly: a backend `{ code: "InvalidRom",
 * details: "…" }` flows through as-is. Other rejections (network/runtime
 * failures) are rethrown unchanged.
 */
async function call<T>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (raw) {
    if (isAppError(raw)) {
      throw raw as AppError;
    }
    throw raw;
  }
}

// ---------------------------------------------------------------------------
// Library group (spec §5.1)
// ---------------------------------------------------------------------------

/** `list_library()` — return the current in-memory library snapshot. */
export function listLibrary(): Promise<LibraryFile> {
  return call<LibraryFile>('list_library');
}

/** `import_rom_from_path(path)` — import an on-disk ROM into the library. */
export function importRomFromPath(path: string): Promise<RomEntry> {
  return call<RomEntry>('import_rom_from_path', { path });
}

/**
 * `import_rom_from_dialog()` — spawn the native file-open dialog and import
 * the user's selection. Resolves to `null` if the user cancelled.
 */
export function importRomFromDialog(): Promise<RomEntry | null> {
  return call<RomEntry | null>('import_rom_from_dialog');
}

/** `remove_rom_from_library(id)` — drop an entry from the library. */
export function removeRomFromLibrary(id: RomId): Promise<void> {
  return call<void>('remove_rom_from_library', { id });
}

/** `rename_rom(id, title)` — update a ROM's display title. */
export function renameRom(id: RomId, title: string): Promise<RomEntry> {
  return call<RomEntry>('rename_rom', { id, title });
}

/** `create_collection(name)` — create a new (uniquely-named) collection. */
export function createCollection(name: string): Promise<Collection> {
  return call<Collection>('create_collection', { name });
}

/** `rename_collection(id, name)` — rename an existing collection. */
export function renameCollection(
  id: CollectionId,
  name: string,
): Promise<Collection> {
  return call<Collection>('rename_collection', { id, name });
}

/** `delete_collection(id)` — drop a collection (ROMs are retained). */
export function deleteCollection(id: CollectionId): Promise<void> {
  return call<void>('delete_collection', { id });
}

/** `add_rom_to_collection(collection, rom)` — append a ROM to a collection. */
export function addRomToCollection(
  collection: CollectionId,
  rom: RomId,
): Promise<void> {
  return call<void>('add_rom_to_collection', { collection, rom });
}

/** `remove_rom_from_collection(collection, rom)` — drop a ROM membership. */
export function removeRomFromCollection(
  collection: CollectionId,
  rom: RomId,
): Promise<void> {
  return call<void>('remove_rom_from_collection', { collection, rom });
}

// ---------------------------------------------------------------------------
// Emulator group (spec §5.1, §5.3)
// ---------------------------------------------------------------------------

/**
 * `start_session(rom, frames)` — start the emulator with `rom` as the source
 * and stream framebuffers through `frames`.
 *
 * `frames` is a Tauri [`Channel`] the caller constructs and registers a
 * message handler on. The runtime emits one message per emulated NTSC frame.
 */
export function startSession(
  rom: RomSource,
  frames: Channel<FrameMessage>,
): Promise<SessionInfo> {
  return call<SessionInfo>('start_session', { rom, frames });
}

/** `stop_session()` — stop the current session (idempotent). */
export function stopSession(): Promise<void> {
  return call<void>('stop_session');
}

/** `set_button_state(player, button, pressed)` — forward an input edge. */
export function setButtonState(
  player: Player,
  button: NesButton,
  pressed: boolean,
): Promise<void> {
  return call<void>('set_button_state', { player, button, pressed });
}

/** `set_paused(paused)` — pause / resume audio + emulation. */
export function setPaused(paused: boolean): Promise<void> {
  return call<void>('set_paused', { paused });
}

/** `set_muted(muted)` — mute / unmute the audio output. */
export function setMuted(muted: boolean): Promise<void> {
  return call<void>('set_muted', { muted });
}

/** `set_volume(volume)` — update the master output volume in `0.0..=1.0`. */
export function setVolume(volume: number): Promise<void> {
  return call<void>('set_volume', { volume });
}

// ---------------------------------------------------------------------------
// Settings group (spec §5.1)
// ---------------------------------------------------------------------------

/** `get_settings()` — return the current persisted [`Settings`] snapshot. */
export function getSettings(): Promise<Settings> {
  return call<Settings>('get_settings');
}

/** `update_bindings(player, map)` — replace one player's [`ButtonMap`]. */
export function updateBindings(
  player: Player,
  map: ButtonMap,
): Promise<Settings> {
  return call<Settings>('update_bindings', { player, map });
}

/** `reset_bindings()` — restore the default key bindings for both players. */
export function resetBindings(): Promise<Settings> {
  return call<Settings>('reset_bindings');
}

// ---------------------------------------------------------------------------
// Shell group (spec §5.1)
// ---------------------------------------------------------------------------

/** `toggle_fullscreen()` — flip the main window's fullscreen flag. */
export function toggleFullscreen(): Promise<FullscreenState> {
  return call<FullscreenState>('toggle_fullscreen');
}

/** `open_external(url)` — hand `url` to the OS default opener. */
export function openExternal(url: string): Promise<void> {
  return call<void>('open_external', { url });
}

/** `log(level, message)` — forward a frontend log line into the Rust log. */
export function log(level: string, message: string): Promise<void> {
  return call<void>('log', { level, message });
}

// Re-export the Tauri `Channel` constructor so callers do not need a
// second import line.
export { Channel };
export type { AppError };
