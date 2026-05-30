// Unit tests for `InputController`.
//
// The controller is exercised against a `document.createElement('div')` as
// the event target so each test starts from a clean listener set. The send
// callback is a `vi.fn` so we can assert call shape directly.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { NesButton, Player } from '../ipc/types';
import {
  DEFAULT_BINDINGS,
  type PlayerBindings,
} from '../store/settingsStore';

import { InputController } from './inputController';

function dispatch(
  target: EventTarget,
  type: 'keydown' | 'keyup',
  code: string,
): void {
  target.dispatchEvent(new KeyboardEvent(type, { code, bubbles: false }));
}

function makeController(
  bindings: PlayerBindings = DEFAULT_BINDINGS,
): {
  controller: InputController;
  send: ReturnType<typeof vi.fn>;
  target: HTMLDivElement;
} {
  const send = vi.fn<
    (player: Player, button: NesButton, pressed: boolean) => Promise<void>
  >().mockResolvedValue(undefined);
  const controller = new InputController(bindings, send);
  const target = document.createElement('div');
  return { controller, send, target };
}

describe('InputController', () => {
  let ctrl: InputController | null = null;

  beforeEach(() => {
    ctrl = null;
  });

  afterEach(() => {
    ctrl?.detach();
  });

  it('fires setButtonState for both P1 and P2 on simultaneous keydowns', () => {
    const { controller, send, target } = makeController();
    ctrl = controller;
    controller.attach(target);

    // KeyJ → P1 "A"; Numpad0 → P2 "A" (per `DEFAULT_BINDINGS`).
    dispatch(target, 'keydown', 'KeyJ');
    dispatch(target, 'keydown', 'Numpad0');

    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenNthCalledWith(1, 1, 'A', true);
    expect(send).toHaveBeenNthCalledWith(2, 2, 'A', true);
  });

  it('emits exactly one transition per keypress despite OS auto-repeat', () => {
    const { controller, send, target } = makeController();
    ctrl = controller;
    controller.attach(target);

    // Three rapid keydowns for the same code (OS auto-repeat).
    dispatch(target, 'keydown', 'KeyJ');
    dispatch(target, 'keydown', 'KeyJ');
    dispatch(target, 'keydown', 'KeyJ');

    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenNthCalledWith(1, 1, 'A', true);

    // After a `keyup` a fresh `keydown` is a new edge.
    dispatch(target, 'keyup', 'KeyJ');
    dispatch(target, 'keydown', 'KeyJ');

    expect(send).toHaveBeenCalledTimes(3);
    expect(send).toHaveBeenNthCalledWith(2, 1, 'A', false);
    expect(send).toHaveBeenNthCalledWith(3, 1, 'A', true);
  });

  it('ignores keys that do not belong to any binding', () => {
    const { controller, send, target } = makeController();
    ctrl = controller;
    controller.attach(target);

    dispatch(target, 'keydown', 'F12');
    dispatch(target, 'keyup', 'F12');

    expect(send).not.toHaveBeenCalled();
  });

  it('detach releases all held keys with a synthetic keyup', () => {
    const { controller, send, target } = makeController();
    ctrl = controller;
    controller.attach(target);

    dispatch(target, 'keydown', 'KeyJ');
    expect(send).toHaveBeenCalledTimes(1);

    controller.detach();
    // The held key is released: one extra send with `pressed=false`.
    expect(send).toHaveBeenCalledTimes(2);
    expect(send).toHaveBeenLastCalledWith(1, 'A', false);

    // After detach, further events on the target are ignored.
    dispatch(target, 'keydown', 'KeyJ');
    expect(send).toHaveBeenCalledTimes(2);
  });

  it('attach is idempotent and isAttached reflects the state', () => {
    const { controller, target } = makeController();
    ctrl = controller;
    expect(controller.isAttached()).toBe(false);
    controller.attach(target);
    expect(controller.isAttached()).toBe(true);
    controller.attach(target); // no-op
    controller.detach();
    expect(controller.isAttached()).toBe(false);
  });

  it('updateBindings retargets subsequent events without reattaching', () => {
    const { controller, send, target } = makeController();
    ctrl = controller;
    controller.attach(target);

    // Swap P1's A button from KeyJ to KeyZ.
    const next: PlayerBindings = {
      ...DEFAULT_BINDINGS,
      p1: { ...DEFAULT_BINDINGS.p1, a: 'KeyZ' },
    };
    controller.updateBindings(next);

    dispatch(target, 'keydown', 'KeyJ');
    expect(send).not.toHaveBeenCalled(); // KeyJ no longer bound
    dispatch(target, 'keydown', 'KeyZ');
    expect(send).toHaveBeenCalledTimes(1);
    expect(send).toHaveBeenLastCalledWith(1, 'A', true);
  });
});
