// Settings client-side state store.
//
// Mirrors `Settings` from `app/src-tauri/src/settings.rs`. Like `libraryStore`,
// this is a pure-reducer zustand store: actions never call IPC; the IPC layer
// (added in a later step) is responsible for round-tripping changes to the
// backend and then pushing the resulting `Settings` here via `setSettings`.

import { create } from 'zustand';

/** Mirrors `ButtonMap` from `app/src-tauri/src/settings.rs`. */
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

/** Mirrors `PlayerBindings` from `app/src-tauri/src/settings.rs`. */
export interface PlayerBindings {
  p1: ButtonMap;
  p2: ButtonMap;
}

/** Mirrors `WindowState` from `app/src-tauri/src/settings.rs`. */
export interface WindowState {
  width: number;
  height: number;
  x: number;
  y: number;
  maximized: boolean;
}

/** Mirrors `Settings` from `app/src-tauri/src/settings.rs`. */
export interface Settings {
  version: number;
  bindings: PlayerBindings;
  volume: number;
  muted: boolean;
  fullscreen_shortcut: string;
  window: WindowState | null;
}

export type Player = 1 | 2;

/**
 * Default key bindings. Kept in sync with `Settings::default()` in
 * `app/src-tauri/src/settings.rs` and with the table in
 * `./.zenflow/tasks/.../spec.md` §4.2.
 *
 * The Rust side remains the source of truth; this client-side copy exists so
 * the UI can render a sensible default before the first IPC round-trip has
 * completed, and so the store tests can exercise reducers without touching
 * the backend.
 */
export const DEFAULT_BINDINGS: PlayerBindings = {
  p1: {
    up: 'KeyW',
    down: 'KeyS',
    left: 'KeyA',
    right: 'KeyD',
    a: 'KeyJ',
    b: 'KeyK',
    start: 'Enter',
    select: 'ShiftRight',
  },
  p2: {
    up: 'ArrowUp',
    down: 'ArrowDown',
    left: 'ArrowLeft',
    right: 'ArrowRight',
    a: 'Numpad0',
    b: 'NumpadDecimal',
    start: 'NumpadEnter',
    select: 'NumpadAdd',
  },
};

export const DEFAULT_SETTINGS: Settings = {
  version: 1,
  bindings: DEFAULT_BINDINGS,
  volume: 1.0,
  muted: false,
  fullscreen_shortcut: 'F11',
  window: null,
};

export interface SettingsState {
  /** Last settings snapshot received from the backend, or `null` before load. */
  settings: Settings | null;
  /** Truthy while a load/save round-trip is in flight. */
  loading: boolean;
  /** Last error message surfaced from the backend, or `null`. */
  error: string | null;

  // Reducer-style actions ---------------------------------------------------
  setSettings: (settings: Settings) => void;
  setLoading: (loading: boolean) => void;
  setError: (error: string | null) => void;
  setBindings: (player: Player, bindings: ButtonMap) => void;
  setVolume: (volume: number) => void;
  setMuted: (muted: boolean) => void;
  setFullscreenShortcut: (shortcut: string) => void;
  resetToDefaults: () => void;
  reset: () => void;
}

const DEFAULT_STATE: Pick<SettingsState, 'settings' | 'loading' | 'error'> = {
  settings: null,
  loading: false,
  error: null,
};

/**
 * Clamp `value` into `[lo, hi]`. Volume is clamped client-side to match the
 * Rust-side validation (`app/src-tauri/src/settings.rs::set_volume`).
 */
const clamp = (value: number, lo: number, hi: number): number =>
  Math.min(hi, Math.max(lo, value));

export const useSettingsStore = create<SettingsState>((set) => ({
  ...DEFAULT_STATE,

  setSettings: (settings) => set({ settings, error: null }),

  setLoading: (loading) => set({ loading }),

  setError: (error) => set({ error }),

  setBindings: (player, bindings) =>
    set((state) => {
      const current = state.settings ?? DEFAULT_SETTINGS;
      const key: 'p1' | 'p2' = player === 1 ? 'p1' : 'p2';
      return {
        settings: {
          ...current,
          bindings: { ...current.bindings, [key]: bindings },
        },
      };
    }),

  setVolume: (volume) =>
    set((state) => {
      const current = state.settings ?? DEFAULT_SETTINGS;
      return { settings: { ...current, volume: clamp(volume, 0, 1) } };
    }),

  setMuted: (muted) =>
    set((state) => {
      const current = state.settings ?? DEFAULT_SETTINGS;
      return { settings: { ...current, muted } };
    }),

  setFullscreenShortcut: (shortcut) =>
    set((state) => {
      const current = state.settings ?? DEFAULT_SETTINGS;
      return { settings: { ...current, fullscreen_shortcut: shortcut } };
    }),

  resetToDefaults: () => set({ settings: DEFAULT_SETTINGS, error: null }),

  reset: () => set({ ...DEFAULT_STATE }),
}));

// -------------------------------------------------------------------------
// Selectors.
// -------------------------------------------------------------------------

export const selectSettings = (state: SettingsState): Settings | null => state.settings;

export const selectBindings =
  (player: Player) =>
  (state: SettingsState): ButtonMap => {
    const current = state.settings ?? DEFAULT_SETTINGS;
    return player === 1 ? current.bindings.p1 : current.bindings.p2;
  };

export const selectVolume = (state: SettingsState): number =>
  state.settings?.volume ?? DEFAULT_SETTINGS.volume;

export const selectMuted = (state: SettingsState): boolean =>
  state.settings?.muted ?? DEFAULT_SETTINGS.muted;

export const selectFullscreenShortcut = (state: SettingsState): string =>
  state.settings?.fullscreen_shortcut ?? DEFAULT_SETTINGS.fullscreen_shortcut;

/**
 * Returns `true` if the given player's bindings contain a duplicate key code
 * — used by the Settings UI to render an inline error without a round-trip.
 */
export const selectHasDuplicateBindings =
  (player: Player) =>
  (state: SettingsState): boolean => {
    const map = selectBindings(player)(state);
    const values = Object.values(map);
    return new Set(values).size !== values.length;
  };
