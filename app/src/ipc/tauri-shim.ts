// Test utilities for mocking the `@tauri-apps/api/core` module via Vitest's
// module mocking primitive.
//
// `./client.ts` and any other frontend module that depends on Tauri IPC
// imports from `@tauri-apps/api/core`. Unit tests do not run inside a Tauri
// shell, so we replace that module with the in-memory shim provided here.
//
// Usage from a test file:
//
// ```ts
// import { vi } from 'vitest';
//
// // The factory uses `await import(...)` so it can reference the shim even
// // though `vi.mock` is hoisted above all static imports.
// vi.mock('@tauri-apps/api/core', async () => {
//   const shim = await import('./tauri-shim');
//   return shim.tauriCoreMockFactory();
// });
//
// import { invokeMock } from './tauri-shim';
// import { listLibrary } from './client';
//
// it('lists the library', async () => {
//   invokeMock.mockResolvedValueOnce({ version: 1, roms: [], collections: [] });
//   await listLibrary();
//   expect(invokeMock).toHaveBeenCalledWith('list_library', undefined);
// });
// ```

import { vi } from 'vitest';

/**
 * Shared mock instance for `invoke`. Tests configure responses on it (via
 * `mockResolvedValueOnce`, `mockRejectedValueOnce`, …) and assert that the
 * wrappers passed the expected command name and arguments.
 */
export const invokeMock = vi.fn<
  (cmd: string, args?: Record<string, unknown>) => unknown
>();

/**
 * Minimal stand-in for the Tauri `Channel<T>` class. The real class registers
 * a callback in a Tauri-managed table; in tests we only need an object that
 * can be serialised and have its `onmessage` invoked manually by the test if
 * it wants to simulate a streamed message.
 */
export class MockChannel<T = unknown> {
  /** Mirrors the real Channel field; defaults to `0` for unit tests. */
  id = 0;
  /** Test-controlled message handler. */
  onmessage?: (response: T) => void;

  constructor(onmessage?: (response: T) => void) {
    if (onmessage) this.onmessage = onmessage;
  }

  /**
   * Real `Channel.toJSON` returns an opaque identifier string. We return an
   * empty string here — tests that introspect `frames` only need a stable
   * non-throwing serialisation.
   */
  toJSON(): string {
    return '';
  }
}

/**
 * Factory whose return value is passed to `vi.mock('@tauri-apps/api/core',
 * factory)` from a test file. Returns an object whose `invoke` delegates to
 * [`invokeMock`] and whose `Channel` is [`MockChannel`].
 */
export function tauriCoreMockFactory(): {
  invoke: (cmd: string, args?: Record<string, unknown>) => unknown;
  Channel: typeof MockChannel;
} {
  return {
    invoke: (cmd, args) => invokeMock(cmd, args),
    Channel: MockChannel,
  };
}

/**
 * Convenience helper to reset the mock between tests. Equivalent to calling
 * `invokeMock.mockReset()` directly; provided so tests can keep their setup
 * blocks short.
 */
export function resetInvokeMock(): void {
  invokeMock.mockReset();
}
