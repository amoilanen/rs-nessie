// Keyboard input controller.
//
// Owns the mapping from `KeyboardEvent.code` strings to NES buttons for both
// player slots (spec §6.3, FR-20/FR-21). A single `keydown` / `keyup` pair of
// listeners attached to the configured event target services both players —
// the controller looks up the originating code in both per-player tables and
// forwards a `setButtonState` IPC call to every match.
//
// The OS auto-repeats `keydown` while a key is held. We track the per-code
// pressed state in a `Map<string, boolean>` and short-circuit subsequent
// `keydown`s for codes that are already known to be pressed, so the IPC layer
// sees one edge per physical keypress (FR-21).

import { setButtonState as defaultSetButtonState } from '../ipc/client';
import type { NesButton, Player } from '../ipc/types';
import type { ButtonMap, PlayerBindings } from '../store/settingsStore';

/**
 * Function signature used by the controller to forward button transitions.
 *
 * Parametrised on injection so tests can substitute a `vi.fn` and so the
 * production `setButtonState` IPC wrapper can be swapped for a different
 * implementation if needed (e.g. local-only mode).
 */
export type SetButtonStateFn = (
  player: Player,
  button: NesButton,
  pressed: boolean,
) => Promise<void>;

/** Order-preserving mapping from a `ButtonMap` key to the wire NES button. */
const BUTTON_TAGS: Record<keyof ButtonMap, NesButton> = {
  up: 'Up',
  down: 'Down',
  left: 'Left',
  right: 'Right',
  a: 'A',
  b: 'B',
  start: 'Start',
  select: 'Select',
};

function invertMap(map: ButtonMap): Map<string, NesButton> {
  const result = new Map<string, NesButton>();
  (Object.keys(BUTTON_TAGS) as (keyof ButtonMap)[]).forEach((key) => {
    result.set(map[key], BUTTON_TAGS[key]);
  });
  return result;
}

/**
 * Single-listener keyboard input controller.
 *
 * The same instance handles both players. `attach(target)` registers two
 * listeners on `target`; `detach()` removes them and releases any keys that
 * were still in the pressed state.
 */
export class InputController {
  private p1: Map<string, NesButton>;
  private p2: Map<string, NesButton>;
  private readonly pressed = new Map<string, boolean>();
  private target: EventTarget | null = null;
  private readonly send: SetButtonStateFn;

  constructor(bindings: PlayerBindings, send: SetButtonStateFn = defaultSetButtonState) {
    this.p1 = invertMap(bindings.p1);
    this.p2 = invertMap(bindings.p2);
    this.send = send;
  }

  /** Replace the active key bindings without detaching listeners. */
  updateBindings(bindings: PlayerBindings): void {
    this.p1 = invertMap(bindings.p1);
    this.p2 = invertMap(bindings.p2);
  }

  /** Returns whether the controller is currently attached to a target. */
  isAttached(): boolean {
    return this.target !== null;
  }

  /**
   * Register the keyboard listeners on `target` (defaults to `window`).
   * Calling `attach` while already attached is a no-op.
   */
  attach(target: EventTarget = window): void {
    if (this.target) return;
    this.target = target;
    target.addEventListener('keydown', this.handleKeyDown as EventListener);
    target.addEventListener('keyup', this.handleKeyUp as EventListener);
  }

  /**
   * Remove the keyboard listeners and release every key that was still
   * pressed (so the emulator does not see a "stuck" button after the view
   * unmounts).
   */
  detach(): void {
    if (!this.target) return;
    const target = this.target;
    target.removeEventListener('keydown', this.handleKeyDown as EventListener);
    target.removeEventListener('keyup', this.handleKeyUp as EventListener);
    this.target = null;
    // Release any held buttons.
    for (const [code, isPressed] of this.pressed) {
      if (isPressed) this.fire(code, false);
    }
    this.pressed.clear();
  }

  private readonly handleKeyDown = (event: KeyboardEvent): void => {
    if (!this.bindsToAny(event.code)) return;
    // Drop OS auto-repeats: only fire on the leading edge.
    if (this.pressed.get(event.code) === true) {
      event.preventDefault();
      return;
    }
    this.pressed.set(event.code, true);
    this.fire(event.code, true);
    event.preventDefault();
  };

  private readonly handleKeyUp = (event: KeyboardEvent): void => {
    if (!this.bindsToAny(event.code)) return;
    // No-op if the corresponding `keydown` was never observed (e.g. focus
    // changes during the keypress).
    if (this.pressed.get(event.code) !== true) return;
    this.pressed.set(event.code, false);
    this.fire(event.code, false);
    event.preventDefault();
  };

  private bindsToAny(code: string): boolean {
    return this.p1.has(code) || this.p2.has(code);
  }

  private fire(code: string, pressed: boolean): void {
    const b1 = this.p1.get(code);
    if (b1) {
      // Promise rejections are swallowed — IPC transport errors are surfaced
      // by the host's own logging and a missed button edge is preferable to
      // crashing the input pipeline.
      this.send(1, b1, pressed).catch(() => {
        /* ignore */
      });
    }
    const b2 = this.p2.get(code);
    if (b2) {
      this.send(2, b2, pressed).catch(() => {
        /* ignore */
      });
    }
  }
}
