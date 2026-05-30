// `KeyBindingRow` — one row of the key bindings table.
//
// Renders the NES button label and two cells (Player 1 / Player 2). Clicking
// a cell enters "capture" mode: the next `keydown` event becomes the new
// binding and is reported through `onCapture`. Pressing `Escape` while in
// capture mode cancels without changing anything.
//
// This component owns only the transient capture state. The actual binding
// values, persistence, and duplicate-detection live in
// `./app/src/routes/SettingsView.tsx`.

import { useEffect, useState, type ReactElement } from 'react';

import type { Player } from '../store/settingsStore';

/** One of the eight NES controller buttons (matches `ButtonMap` keys). */
export type NesButtonKey =
  | 'up'
  | 'down'
  | 'left'
  | 'right'
  | 'a'
  | 'b'
  | 'start'
  | 'select';

export interface KeyBindingRowProps {
  /** The NES button this row represents. */
  button: NesButtonKey;
  /** Human-readable label rendered in the leading cell (e.g. "Up"). */
  label: string;
  /** `KeyboardEvent.code` currently bound to this button for Player 1. */
  p1Code: string;
  /** `KeyboardEvent.code` currently bound to this button for Player 2. */
  p2Code: string;
  /**
   * Invoked with the new `KeyboardEvent.code` once the user finishes capture.
   * Not called when the user presses `Escape` (capture is cancelled silently).
   */
  onCapture: (player: Player, button: NesButtonKey, code: string) => void;
  /** When `true`, render the P1 cell with a conflict outline. */
  p1Conflict?: boolean;
  /** When `true`, render the P2 cell with a conflict outline. */
  p2Conflict?: boolean;
}

type CaptureSide = 'p1' | 'p2' | null;

export function KeyBindingRow({
  button,
  label,
  p1Code,
  p2Code,
  onCapture,
  p1Conflict = false,
  p2Conflict = false,
}: KeyBindingRowProps): ReactElement {
  const [capturing, setCapturing] = useState<CaptureSide>(null);

  useEffect(() => {
    if (capturing === null) return;
    // `active` self-disables the handler after the first consumed event so
    // any keydowns dispatched before React has actually torn down the
    // listener (state updates are async) do not register a second time.
    let active = true;
    const handler = (e: KeyboardEvent): void => {
      if (!active) return;
      if (e.key === 'Escape') {
        active = false;
        e.preventDefault();
        setCapturing(null);
        return;
      }
      // Ignore key events that don't carry a usable physical code — these
      // come from synthesized IME events or unusual input methods and would
      // produce a meaningless binding string.
      if (!e.code || e.code === 'Unidentified') return;
      active = false;
      e.preventDefault();
      e.stopPropagation();
      const player: Player = capturing === 'p1' ? 1 : 2;
      onCapture(player, button, e.code);
      setCapturing(null);
    };
    window.addEventListener('keydown', handler);
    return (): void => {
      active = false;
      window.removeEventListener('keydown', handler);
    };
  }, [capturing, button, onCapture]);

  const cellClass = (
    side: 'p1' | 'p2',
    conflict: boolean,
  ): string => {
    const classes = ['key-binding-row__cell'];
    if (capturing === side) classes.push('is-capturing');
    if (conflict) classes.push('is-conflict');
    return classes.join(' ');
  };

  const toggle = (side: 'p1' | 'p2'): void => {
    setCapturing((prev) => (prev === side ? null : side));
  };

  return (
    <tr className="key-binding-row" data-button={button}>
      <th scope="row" className="key-binding-row__label">
        {label}
      </th>
      <td>
        <button
          type="button"
          className={cellClass('p1', p1Conflict)}
          aria-label={`Player 1 ${label} binding`}
          aria-pressed={capturing === 'p1'}
          onClick={(): void => toggle('p1')}
        >
          {capturing === 'p1' ? 'Press a key… (Esc to cancel)' : p1Code}
        </button>
      </td>
      <td>
        <button
          type="button"
          className={cellClass('p2', p2Conflict)}
          aria-label={`Player 2 ${label} binding`}
          aria-pressed={capturing === 'p2'}
          onClick={(): void => toggle('p2')}
        >
          {capturing === 'p2' ? 'Press a key… (Esc to cancel)' : p2Code}
        </button>
      </td>
    </tr>
  );
}
