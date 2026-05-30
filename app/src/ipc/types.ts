// Typed IPC contract between the React frontend and the Rust Tauri host.
//
// Every type in this file mirrors a Rust counterpart defined in
// `./app/src-tauri/src/{error,library,settings,commands/*}.rs`. The naming
// convention deliberately preserves the snake_case field names emitted by
// `serde` so JSON payloads round-trip without an extra transformation layer
// in either direction.
//
// Spec references:
// - §5.1 (IPC commands and shared types: `RomEntry`, `Collection`, …,
//   `RomSource`, `NesButton`, `FrameMessage`, `Settings`, `ButtonMap`)
// - §5.2 (`AppError` discriminated union — wire shape
//   `{ "code": "...", "details": ... }`)

/** Stable identifier for a ROM (UUID v4 string). */
export type RomId = string;

/** Stable identifier for a collection (UUID v4 string). */
export type CollectionId = string;

/**
 * A ROM as stored in the user's library.
 * Mirrors `RomEntry` in `./app/src-tauri/src/library.rs`.
 */
export interface RomEntry {
  /** Unique, persistent identifier (UUID v4). */
  id: RomId;
  /** User-editable display title. Defaults to the file stem on import. */
  title: string;
  /** Absolute path to the ROM on disk. */
  path: string;
  /** SHA-1 of the ROM bytes (lowercase hex, 40 chars). */
  sha1: string;
  /** iNES mapper number. */
  mapper: number;
  /** Length of the ROM in bytes. */
  size_bytes: number;
  /** Unix milliseconds when the ROM was first imported. */
  imported_at: number;
}

/**
 * A user-defined grouping of ROMs.
 * Mirrors `Collection` in `./app/src-tauri/src/library.rs`.
 */
export interface Collection {
  /** Unique, persistent identifier (UUID v4). */
  id: CollectionId;
  /** Human-readable name (unique per library). */
  name: string;
  /** Ordered list of ROM ids belonging to this collection. */
  rom_ids: RomId[];
  /** Unix milliseconds when the collection was created. */
  created_at: number;
}

/**
 * On-disk shape of `library.json`.
 * Mirrors `LibraryFile` in `./app/src-tauri/src/library.rs`.
 */
export interface LibraryFile {
  /** Schema version. */
  version: number;
  /** All ROMs known to the user. */
  roms: RomEntry[];
  /** All user-defined collections. */
  collections: Collection[];
}

/**
 * Mapping from one NES controller's eight buttons to `KeyboardEvent.code`
 * values. Mirrors `ButtonMap` in `./app/src-tauri/src/settings.rs`.
 */
export interface ButtonMap {
  up: string;
  down: string;
  left: string;
  right: string;
  a: string;
  b: string;
  start: string;
  select: string;
}

/**
 * Per-player [`ButtonMap`] pair.
 * Mirrors `PlayerBindings` in `./app/src-tauri/src/settings.rs`.
 */
export interface PlayerBindings {
  /** Player 1 button → key code mapping. */
  p1: ButtonMap;
  /** Player 2 button → key code mapping. */
  p2: ButtonMap;
}

/**
 * Last known window size and position.
 * Mirrors `WindowState` in `./app/src-tauri/src/settings.rs`.
 */
export interface WindowState {
  /** Window width in CSS pixels. */
  width: number;
  /** Window height in CSS pixels. */
  height: number;
  /** Top-left X in screen-space CSS pixels. */
  x: number;
  /** Top-left Y in screen-space CSS pixels. */
  y: number;
  /** Whether the window was maximized when last closed. */
  maximized: boolean;
}

/**
 * Persisted user settings.
 * Mirrors `Settings` in `./app/src-tauri/src/settings.rs`.
 */
export interface Settings {
  /** Schema version. */
  version: number;
  /** Key bindings for both players. */
  bindings: PlayerBindings;
  /** Master output volume in `0.0..=1.0`. */
  volume: number;
  /** Whether audio is muted. */
  muted: boolean;
  /** Keyboard shortcut that toggles fullscreen mode (e.g. `"F11"`). */
  fullscreen_shortcut: string;
  /** Last known window placement, or `null` to use the platform default. */
  window: WindowState | null;
}

/**
 * Player slot used by IPC calls that operate on a single controller. The
 * Rust host expects the literal numeric values `1` and `2`.
 */
export type Player = 1 | 2;

/**
 * NES button names matching the [`Button`] enum exposed by `nessie-core`
 * (spec §5.1). Sent as strings over the wire.
 */
export type NesButton =
  | 'A'
  | 'B'
  | 'Up'
  | 'Down'
  | 'Left'
  | 'Right'
  | 'Start'
  | 'Select';

/**
 * Discriminated-union ROM source for `startSession`.
 * Mirrors `RomSource` in `./app/src-tauri/src/commands/emulator.rs`
 * (`#[serde(tag = "kind", rename_all = "lowercase")]`).
 */
export type RomSource =
  | { kind: 'library'; id: RomId }
  | { kind: 'path'; path: string };

/**
 * Snapshot returned by `startSession` so the HUD can render before the first
 * frame arrives. Mirrors `SessionInfo` in
 * `./app/src-tauri/src/commands/emulator.rs`.
 */
export interface SessionInfo {
  /** SHA-1 of the loaded cartridge. */
  sha1: string;
  /** `true` if the cartridge has battery-backed PRG-RAM. */
  has_battery: boolean;
  /** iNES mapper number. */
  mapper: number;
}

/**
 * Streamed framebuffer message (spec §5.1 / §5.3).
 *
 * The Rust runtime currently sends raw bytes prefixed with the little-endian
 * frame index (see `ChannelFrameSink` in
 * `./app/src-tauri/src/commands/emulator.rs`). The frontend session module
 * decodes that payload into this typed shape before handing the pixel buffer
 * to the WebGL2 renderer.
 */
export interface FrameMessage {
  /** Monotonic frame index (NES frame counter). */
  frame: number;
  /** 256×240 RGBA8 pixels (245_760 bytes). */
  pixels: ArrayBuffer;
}

/**
 * Response of `toggleFullscreen`.
 * Mirrors `FullscreenState` in `./app/src-tauri/src/commands/shell.rs`.
 */
export interface FullscreenState {
  /** `true` if the window is now fullscreen after the toggle. */
  fullscreen: boolean;
}

/**
 * Typed application error envelope (spec §5.2). Returned as the `Err` arm of
 * every `Result<T, AppError>` in `./app/src-tauri/src/error.rs`. Tauri
 * surfaces it through the rejected promise of `invoke`; the wrapper functions
 * in `./client.ts` rethrow these values unchanged so callers can `switch` on
 * the `code` field.
 *
 * Wire shape: `{ "code": "InvalidRom", "details": "missing magic" }`.
 * `NotFound` has no `details` field.
 */
export type AppError =
  | { code: 'InvalidRom'; details: string }
  | { code: 'UnsupportedMapper'; details: number }
  | { code: 'RomMissing'; details: string }
  | { code: 'LibraryCorrupted'; details: string }
  | { code: 'NotFound' }
  | { code: 'Io'; details: string };

/** All `AppError.code` discriminator values, for runtime checks. */
export const APP_ERROR_CODES = [
  'InvalidRom',
  'UnsupportedMapper',
  'RomMissing',
  'LibraryCorrupted',
  'NotFound',
  'Io',
] as const;

/** Narrow an `unknown` value into a typed [`AppError`]. */
export function isAppError(value: unknown): value is AppError {
  if (typeof value !== 'object' || value === null) return false;
  const code = (value as { code?: unknown }).code;
  if (typeof code !== 'string') return false;
  return (APP_ERROR_CODES as readonly string[]).includes(code);
}
