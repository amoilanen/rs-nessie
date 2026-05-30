// Component tests for the `SettingsView` route.
//
// The IPC client is mocked. The settings store is reset between tests.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { Settings } from '../store/settingsStore';
import { DEFAULT_SETTINGS, useSettingsStore } from '../store/settingsStore';
import { useToastStore } from '../store/toastStore';

vi.mock('../ipc/client', () => ({
  getSettings: vi.fn(),
  updateBindings: vi.fn(),
  resetBindings: vi.fn(),
  setVolume: vi.fn(),
  setMuted: vi.fn(),
}));

import {
  getSettings,
  resetBindings,
  setMuted,
  setVolume,
  updateBindings,
} from '../ipc/client';

import { SettingsView } from './SettingsView';

const cloneDefaults = (): Settings =>
  JSON.parse(JSON.stringify(DEFAULT_SETTINGS)) as Settings;

/** Slightly longer than the debounce in `SettingsView.tsx`. */
const DEBOUNCE_FLUSH_MS = 350;

const sleep = (ms: number): Promise<void> =>
  new Promise((resolve) => setTimeout(resolve, ms));

beforeEach(() => {
  vi.mocked(getSettings).mockResolvedValue(cloneDefaults());
  vi.mocked(updateBindings).mockResolvedValue(cloneDefaults());
  vi.mocked(resetBindings).mockResolvedValue(cloneDefaults());
  vi.mocked(setVolume).mockResolvedValue(undefined);
  vi.mocked(setMuted).mockResolvedValue(undefined);
});

afterEach(() => {
  vi.clearAllMocks();
  useSettingsStore.getState().reset();
  useToastStore.getState().clear();
});

async function waitForSettingsLoaded(): Promise<void> {
  await waitFor(() => {
    expect(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    ).toBeInTheDocument();
  });
}

describe('SettingsView', () => {
  it('renders the bindings table once settings have loaded', async () => {
    render(<SettingsView />);
    await waitForSettingsLoaded();
    expect(getSettings).toHaveBeenCalledTimes(1);
    // Default P1 A binding is "KeyJ".
    expect(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    ).toHaveTextContent('KeyJ');
    // Default P2 Up binding is "ArrowUp".
    expect(
      screen.getByRole('button', { name: 'Player 2 Up binding' }),
    ).toHaveTextContent('ArrowUp');
  });

  it('shows an inline duplicate-binding error without invoking IPC', async () => {
    render(<SettingsView />);
    await waitForSettingsLoaded();

    // P1 "Up" is bound to "KeyW" by default. Assigning "KeyW" to P1 "A"
    // creates a duplicate within Player 1's map.
    fireEvent.click(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    );
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyW' }));

    await waitFor(() => {
      expect(screen.getByRole('alert')).toHaveTextContent(
        /Player 1 has duplicate key bindings/i,
      );
    });

    // Wait past the debounce window — even after the timer would have
    // fired we must not have called updateBindings for the duplicate change.
    await sleep(DEBOUNCE_FLUSH_MS);
    expect(updateBindings).not.toHaveBeenCalled();
  });

  it('debounces and persists a valid binding change', async () => {
    render(<SettingsView />);
    await waitForSettingsLoaded();

    // "KeyP" is not used by any default P1 binding.
    fireEvent.click(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    );
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyP' }));

    // Before the debounce window elapses, no IPC call should have fired.
    expect(updateBindings).not.toHaveBeenCalled();

    await waitFor(() => {
      expect(updateBindings).toHaveBeenCalledTimes(1);
    });
    const [player, map] = vi.mocked(updateBindings).mock.calls[0]!;
    expect(player).toBe(1);
    expect(map.a).toBe('KeyP');
  });

  it('resets a player to its default bindings', async () => {
    render(<SettingsView />);
    await waitForSettingsLoaded();

    fireEvent.click(
      screen.getByRole('button', { name: 'Reset Player 1 to defaults' }),
    );
    await waitFor(() => {
      expect(updateBindings).toHaveBeenCalledTimes(1);
    });
    const [player, map] = vi.mocked(updateBindings).mock.calls[0]!;
    expect(player).toBe(1);
    // The submitted map equals the documented defaults.
    expect(map).toEqual(DEFAULT_SETTINGS.bindings.p1);
  });

  it('invokes resetBindings when the global reset button is clicked', async () => {
    render(<SettingsView />);
    await waitForSettingsLoaded();

    fireEvent.click(
      screen.getByRole('button', { name: 'Reset all bindings to defaults' }),
    );
    await waitFor(() => {
      expect(resetBindings).toHaveBeenCalledTimes(1);
    });
  });
});
