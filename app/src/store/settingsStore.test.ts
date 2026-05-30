import { afterEach, describe, expect, it } from 'vitest';

import type { ButtonMap, Settings } from './settingsStore';
import {
  DEFAULT_BINDINGS,
  DEFAULT_SETTINGS,
  selectBindings,
  selectFullscreenShortcut,
  selectHasDuplicateBindings,
  selectMuted,
  selectSettings,
  selectVolume,
  useSettingsStore,
} from './settingsStore';

const cloneDefaults = (): Settings => JSON.parse(JSON.stringify(DEFAULT_SETTINGS)) as Settings;

afterEach(() => {
  useSettingsStore.getState().reset();
});

describe('settingsStore reducers', () => {
  it('starts in an empty default state', () => {
    const state = useSettingsStore.getState();
    expect(state.settings).toBeNull();
    expect(state.loading).toBe(false);
    expect(state.error).toBeNull();
  });

  it('setSettings replaces the snapshot and clears the error', () => {
    useSettingsStore.getState().setError('previous');
    const snapshot = cloneDefaults();
    useSettingsStore.getState().setSettings(snapshot);
    const state = useSettingsStore.getState();
    expect(state.settings).toEqual(snapshot);
    expect(state.error).toBeNull();
  });

  it('setBindings updates only the requested player', () => {
    useSettingsStore.getState().setSettings(cloneDefaults());
    const custom: ButtonMap = {
      up: 'KeyT',
      down: 'KeyG',
      left: 'KeyF',
      right: 'KeyH',
      a: 'KeyZ',
      b: 'KeyX',
      start: 'Tab',
      select: 'Backquote',
    };
    useSettingsStore.getState().setBindings(1, custom);
    const after = useSettingsStore.getState().settings!;
    expect(after.bindings.p1).toEqual(custom);
    expect(after.bindings.p2).toEqual(DEFAULT_BINDINGS.p2);
  });

  it('setVolume clamps to [0, 1]', () => {
    useSettingsStore.getState().setSettings(cloneDefaults());
    useSettingsStore.getState().setVolume(2.5);
    expect(useSettingsStore.getState().settings?.volume).toBe(1);
    useSettingsStore.getState().setVolume(-1);
    expect(useSettingsStore.getState().settings?.volume).toBe(0);
    useSettingsStore.getState().setVolume(0.42);
    expect(useSettingsStore.getState().settings?.volume).toBeCloseTo(0.42);
  });

  it('setMuted toggles the muted flag', () => {
    useSettingsStore.getState().setSettings(cloneDefaults());
    useSettingsStore.getState().setMuted(true);
    expect(useSettingsStore.getState().settings?.muted).toBe(true);
    useSettingsStore.getState().setMuted(false);
    expect(useSettingsStore.getState().settings?.muted).toBe(false);
  });

  it('setFullscreenShortcut updates the shortcut string', () => {
    useSettingsStore.getState().setSettings(cloneDefaults());
    useSettingsStore.getState().setFullscreenShortcut('F12');
    expect(useSettingsStore.getState().settings?.fullscreen_shortcut).toBe('F12');
  });

  it('resetToDefaults restores the documented defaults', () => {
    useSettingsStore.getState().setSettings({
      ...cloneDefaults(),
      volume: 0.1,
      muted: true,
    });
    useSettingsStore.getState().resetToDefaults();
    expect(useSettingsStore.getState().settings).toEqual(DEFAULT_SETTINGS);
  });

  it('reducers operate on DEFAULT_SETTINGS when no snapshot has loaded', () => {
    // Before any backend round-trip we still want UI controls to work.
    useSettingsStore.getState().setVolume(0.25);
    expect(useSettingsStore.getState().settings?.volume).toBe(0.25);
    expect(useSettingsStore.getState().settings?.fullscreen_shortcut).toBe('F11');
  });
});

describe('settingsStore selectors', () => {
  it('fall back to DEFAULT_SETTINGS when nothing has loaded', () => {
    const state = useSettingsStore.getState();
    expect(selectSettings(state)).toBeNull();
    expect(selectVolume(state)).toBe(DEFAULT_SETTINGS.volume);
    expect(selectMuted(state)).toBe(DEFAULT_SETTINGS.muted);
    expect(selectFullscreenShortcut(state)).toBe(DEFAULT_SETTINGS.fullscreen_shortcut);
    expect(selectBindings(1)(state)).toEqual(DEFAULT_BINDINGS.p1);
    expect(selectBindings(2)(state)).toEqual(DEFAULT_BINDINGS.p2);
  });

  it('selectBindings returns the requested player slice', () => {
    useSettingsStore.getState().setSettings(cloneDefaults());
    expect(selectBindings(1)(useSettingsStore.getState())).toEqual(DEFAULT_BINDINGS.p1);
    expect(selectBindings(2)(useSettingsStore.getState())).toEqual(DEFAULT_BINDINGS.p2);
  });

  it('selectHasDuplicateBindings reports collisions within a player', () => {
    useSettingsStore.getState().setSettings(cloneDefaults());
    expect(selectHasDuplicateBindings(1)(useSettingsStore.getState())).toBe(false);
    useSettingsStore.getState().setBindings(1, {
      ...DEFAULT_BINDINGS.p1,
      // Force a duplicate key code.
      a: DEFAULT_BINDINGS.p1.up,
    });
    expect(selectHasDuplicateBindings(1)(useSettingsStore.getState())).toBe(true);
    // P2 untouched.
    expect(selectHasDuplicateBindings(2)(useSettingsStore.getState())).toBe(false);
  });
});
