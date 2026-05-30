// Unit tests for the `KeyBindingRow` component.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { describe, expect, it, vi } from 'vitest';

import { KeyBindingRow } from './KeyBindingRow';

function renderRow(
  overrides: Partial<ComponentProps<typeof KeyBindingRow>> = {},
): { onCapture: ReturnType<typeof vi.fn> } {
  const onCapture = vi.fn();
  render(
    <table>
      <tbody>
        <KeyBindingRow
          button="a"
          label="A"
          p1Code="KeyJ"
          p2Code="Numpad0"
          onCapture={onCapture}
          {...overrides}
        />
      </tbody>
    </table>,
  );
  return { onCapture };
}

describe('KeyBindingRow', () => {
  it('renders the current bindings for both players', () => {
    renderRow();
    const p1 = screen.getByRole('button', { name: 'Player 1 A binding' });
    const p2 = screen.getByRole('button', { name: 'Player 2 A binding' });
    expect(p1).toHaveTextContent('KeyJ');
    expect(p2).toHaveTextContent('Numpad0');
  });

  it('captures the next keydown after entering capture mode (P1)', () => {
    const { onCapture } = renderRow();
    fireEvent.click(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    );
    // While capturing, the cell label switches to the prompt text.
    expect(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    ).toHaveTextContent(/Press a key/i);
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ' }));
    expect(onCapture).toHaveBeenCalledTimes(1);
    expect(onCapture).toHaveBeenCalledWith(1, 'a', 'KeyZ');
  });

  it('captures the next keydown for P2 independently', () => {
    const { onCapture } = renderRow();
    fireEvent.click(
      screen.getByRole('button', { name: 'Player 2 A binding' }),
    );
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'Backquote' }));
    expect(onCapture).toHaveBeenCalledTimes(1);
    expect(onCapture).toHaveBeenCalledWith(2, 'a', 'Backquote');
  });

  it('cancels capture mode on Escape without emitting', async () => {
    const { onCapture } = renderRow();
    fireEvent.click(
      screen.getByRole('button', { name: 'Player 1 A binding' }),
    );
    window.dispatchEvent(
      new KeyboardEvent('keydown', { key: 'Escape', code: 'Escape' }),
    );
    expect(onCapture).not.toHaveBeenCalled();
    // After cancel, additional keydowns must not register either.
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ' }));
    expect(onCapture).not.toHaveBeenCalled();
    // Cell returns to displaying the bound key once React flushes the state
    // update issued from inside the dispatched event handler.
    await waitFor(() => {
      expect(
        screen.getByRole('button', { name: 'Player 1 A binding' }),
      ).toHaveTextContent('KeyJ');
    });
  });

  it('toggling the same cell exits capture mode', () => {
    const { onCapture } = renderRow();
    const cell = screen.getByRole('button', { name: 'Player 1 A binding' });
    fireEvent.click(cell);
    expect(cell).toHaveTextContent(/Press a key/i);
    fireEvent.click(cell);
    expect(cell).toHaveTextContent('KeyJ');
    window.dispatchEvent(new KeyboardEvent('keydown', { code: 'KeyZ' }));
    // The listener should have been torn down — no capture happens.
    expect(onCapture).not.toHaveBeenCalled();
  });

  it('renders a conflict marker when the corresponding prop is true', () => {
    renderRow({ p1Conflict: true });
    const cell = screen.getByRole('button', { name: 'Player 1 A binding' });
    expect(cell.className).toContain('is-conflict');
  });
});
