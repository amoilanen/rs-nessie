// Settings route.
//
// Three sections:
//   1. Key bindings — a table with one row per `NesButton` and two columns
//      (Player 1 / Player 2). Cells enter "capture" mode on click; the next
//      `keydown` becomes the new binding. Successful changes are persisted
//      via a debounced `updateBindings` IPC call. Duplicate bindings within
//      a single player render an inline error and are NOT sent to the
//      backend (the Rust host would reject them anyway — we just shortcut
//      that round-trip for snappier feedback, per FR-22/23/24 and spec §4.2).
//   2. Audio — master volume slider and mute toggle. Changes are mirrored to
//      the running session via the emulator IPC group.
//   3. General — per-player and global "Reset to defaults" buttons.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
} from 'react';

import { KeyBindingRow, type NesButtonKey } from '../components/KeyBindingRow';
import { VolumeSlider } from '../components/VolumeSlider';
import {
  getSettings,
  resetBindings,
  setMuted as ipcSetMuted,
  setVolume as ipcSetVolume,
  updateBindings,
} from '../ipc/client';
import { formatAppError } from '../ipc/format';
import {
  DEFAULT_BINDINGS,
  DEFAULT_SETTINGS,
  useSettingsStore,
  type ButtonMap,
  type Player,
} from '../store/settingsStore';
import { pushErrorToast } from '../store/toastStore';

/** Display order and labels for the eight NES controller buttons. */
const BUTTON_ROWS: { button: NesButtonKey; label: string }[] = [
  { button: 'up', label: 'Up' },
  { button: 'down', label: 'Down' },
  { button: 'left', label: 'Left' },
  { button: 'right', label: 'Right' },
  { button: 'a', label: 'A' },
  { button: 'b', label: 'B' },
  { button: 'start', label: 'Start' },
  { button: 'select', label: 'Select' },
];

/** Debounce window for save IPC calls (ms). */
const SAVE_DEBOUNCE_MS = 250;

/**
 * Build a `code → count` lookup so the UI can mark every cell that shares a
 * duplicated key code as conflicting (rather than only the most recently
 * changed cell).
 */
function buildCodeCounts(map: ButtonMap): Map<string, number> {
  const counts = new Map<string, number>();
  for (const code of Object.values(map)) {
    counts.set(code, (counts.get(code) ?? 0) + 1);
  }
  return counts;
}

/** Returns `true` if any `code` appears more than once in `map`. */
function hasDuplicates(map: ButtonMap): boolean {
  const values = Object.values(map);
  return new Set(values).size !== values.length;
}

export function SettingsView(): ReactElement {
  const settings = useSettingsStore((s) => s.settings);
  const setSettings = useSettingsStore((s) => s.setSettings);
  const setLoading = useSettingsStore((s) => s.setLoading);
  const setError = useSettingsStore((s) => s.setError);
  const setStoreBindings = useSettingsStore((s) => s.setBindings);
  const setStoreVolume = useSettingsStore((s) => s.setVolume);
  const setStoreMuted = useSettingsStore((s) => s.setMuted);

  // Local mirror of the editable values. Initialized from the documented
  // defaults so the controls remain interactive before the first IPC
  // round-trip completes.
  const [p1, setP1] = useState<ButtonMap>(DEFAULT_BINDINGS.p1);
  const [p2, setP2] = useState<ButtonMap>(DEFAULT_BINDINGS.p2);
  const [volume, setVolumeLocal] = useState<number>(DEFAULT_SETTINGS.volume);
  const [muted, setMutedLocal] = useState<boolean>(DEFAULT_SETTINGS.muted);
  const [p1HasDuplicate, setP1HasDuplicate] = useState(false);
  const [p2HasDuplicate, setP2HasDuplicate] = useState(false);

  const bindingTimers = useRef<{
    p1: ReturnType<typeof setTimeout> | null;
    p2: ReturnType<typeof setTimeout> | null;
  }>({ p1: null, p2: null });
  const volumeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  // Fetch the persisted settings snapshot once on mount.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    getSettings()
      .then((s) => {
        if (cancelled) return;
        setSettings(s);
      })
      .catch((err) => {
        if (cancelled) return;
        const message = formatAppError(err);
        setError(message);
        pushErrorToast(message);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return (): void => {
      cancelled = true;
    };
  }, [setSettings, setLoading, setError]);

  // Reflect external snapshot changes (initial load, reset, etc.) into the
  // local editable copy.
  useEffect(() => {
    if (!settings) return;
    setP1(settings.bindings.p1);
    setP2(settings.bindings.p2);
    setVolumeLocal(settings.volume);
    setMutedLocal(settings.muted);
    setP1HasDuplicate(hasDuplicates(settings.bindings.p1));
    setP2HasDuplicate(hasDuplicates(settings.bindings.p2));
  }, [settings]);

  // Always clear any pending debounce timers when the view unmounts.
  useEffect(() => {
    const timers = bindingTimers.current;
    const vTimer = volumeTimer;
    return (): void => {
      if (timers.p1) clearTimeout(timers.p1);
      if (timers.p2) clearTimeout(timers.p2);
      if (vTimer.current) clearTimeout(vTimer.current);
    };
  }, []);

  const scheduleSaveBindings = useCallback(
    (player: Player, map: ButtonMap): void => {
      const key: 'p1' | 'p2' = player === 1 ? 'p1' : 'p2';
      const existing = bindingTimers.current[key];
      if (existing) clearTimeout(existing);
      bindingTimers.current[key] = setTimeout(() => {
        bindingTimers.current[key] = null;
        updateBindings(player, map)
          .then((updated) => setSettings(updated))
          .catch((err) => pushErrorToast(formatAppError(err)));
      }, SAVE_DEBOUNCE_MS);
    },
    [setSettings],
  );

  const cancelPendingSave = useCallback((player: Player): void => {
    const key: 'p1' | 'p2' = player === 1 ? 'p1' : 'p2';
    const existing = bindingTimers.current[key];
    if (existing) {
      clearTimeout(existing);
      bindingTimers.current[key] = null;
    }
  }, []);

  const handleCapture = useCallback(
    (player: Player, button: NesButtonKey, code: string): void => {
      const current = player === 1 ? p1 : p2;
      const nextMap: ButtonMap = { ...current, [button]: code };
      const duplicate = hasDuplicates(nextMap);

      if (player === 1) {
        setP1(nextMap);
        setP1HasDuplicate(duplicate);
      } else {
        setP2(nextMap);
        setP2HasDuplicate(duplicate);
      }
      setStoreBindings(player, nextMap);

      if (duplicate) {
        // The Rust host would reject this update — short-circuit the IPC
        // round-trip so the user sees the inline error immediately.
        cancelPendingSave(player);
        return;
      }
      scheduleSaveBindings(player, nextMap);
    },
    [p1, p2, setStoreBindings, scheduleSaveBindings, cancelPendingSave],
  );

  const handleVolume = useCallback(
    (next: number): void => {
      setVolumeLocal(next);
      setStoreVolume(next);
      if (volumeTimer.current) clearTimeout(volumeTimer.current);
      volumeTimer.current = setTimeout(() => {
        volumeTimer.current = null;
        ipcSetVolume(next).catch((err) =>
          pushErrorToast(formatAppError(err)),
        );
      }, SAVE_DEBOUNCE_MS);
    },
    [setStoreVolume],
  );

  const handleMuted = useCallback(
    (next: boolean): void => {
      setMutedLocal(next);
      setStoreMuted(next);
      ipcSetMuted(next).catch((err) => pushErrorToast(formatAppError(err)));
    },
    [setStoreMuted],
  );

  const handleResetPlayer = useCallback(
    (player: Player): void => {
      const defaults =
        player === 1 ? DEFAULT_BINDINGS.p1 : DEFAULT_BINDINGS.p2;
      if (player === 1) {
        setP1(defaults);
        setP1HasDuplicate(false);
      } else {
        setP2(defaults);
        setP2HasDuplicate(false);
      }
      setStoreBindings(player, defaults);
      scheduleSaveBindings(player, defaults);
    },
    [setStoreBindings, scheduleSaveBindings],
  );

  const handleResetAll = useCallback((): void => {
    // Cancel any pending per-player saves before issuing a global reset so we
    // don't race the reset response.
    cancelPendingSave(1);
    cancelPendingSave(2);
    resetBindings()
      .then((updated) => setSettings(updated))
      .catch((err) => pushErrorToast(formatAppError(err)));
  }, [cancelPendingSave, setSettings]);

  const p1Counts = useMemo(() => buildCodeCounts(p1), [p1]);
  const p2Counts = useMemo(() => buildCodeCounts(p2), [p2]);

  return (
    <section className="settings-view" aria-labelledby="settings-heading">
      <h1 id="settings-heading">Settings</h1>

      <section
        aria-labelledby="settings-bindings-heading"
        className="settings-view__section"
      >
        <h2 id="settings-bindings-heading">Key bindings</h2>
        <table className="key-bindings-table">
          <thead>
            <tr>
              <th scope="col">Button</th>
              <th scope="col">Player 1</th>
              <th scope="col">Player 2</th>
            </tr>
          </thead>
          <tbody>
            {BUTTON_ROWS.map((row) => {
              const p1Code = p1[row.button];
              const p2Code = p2[row.button];
              const rowProps = {
                button: row.button,
                label: row.label,
                p1Code,
                p2Code,
                onCapture: handleCapture,
                p1Conflict: (p1Counts.get(p1Code) ?? 0) > 1,
                p2Conflict: (p2Counts.get(p2Code) ?? 0) > 1,
              };
              return <KeyBindingRow key={row.button} {...rowProps} />;
            })}
          </tbody>
        </table>
        {p1HasDuplicate ? (
          <p
            className="settings-view__error"
            role="alert"
            data-player="1"
          >
            Player 1 has duplicate key bindings. Please assign a unique key
            to each button.
          </p>
        ) : null}
        {p2HasDuplicate ? (
          <p
            className="settings-view__error"
            role="alert"
            data-player="2"
          >
            Player 2 has duplicate key bindings. Please assign a unique key
            to each button.
          </p>
        ) : null}
        <div className="settings-view__actions">
          <button
            type="button"
            className="button button--ghost"
            onClick={(): void => handleResetPlayer(1)}
          >
            Reset Player 1 to defaults
          </button>
          <button
            type="button"
            className="button button--ghost"
            onClick={(): void => handleResetPlayer(2)}
          >
            Reset Player 2 to defaults
          </button>
        </div>
      </section>

      <section
        aria-labelledby="settings-audio-heading"
        className="settings-view__section"
      >
        <h2 id="settings-audio-heading">Audio</h2>
        <VolumeSlider
          volume={volume}
          muted={muted}
          onVolumeChange={handleVolume}
          onMutedChange={handleMuted}
        />
      </section>

      <section
        aria-labelledby="settings-general-heading"
        className="settings-view__section"
      >
        <h2 id="settings-general-heading">General</h2>
        <button
          type="button"
          className="button button--danger"
          onClick={handleResetAll}
        >
          Reset all bindings to defaults
        </button>
      </section>
    </section>
  );
}
