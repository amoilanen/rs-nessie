// Component tests for the `GameView` route.
//
// The view's heavy lifting (WebGL, IPC channel, key listeners) is delegated
// to `mountGameSession`; tests mock that single seam so we can verify the
// lifecycle without touching WebGL or Tauri.

import { render, waitFor } from '@testing-library/react';
import { MemoryRouter, Route, Routes } from 'react-router-dom';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type { GameSession } from '../emu/session';
import type { SessionInfo } from '../ipc/types';

vi.mock('../emu/session', () => ({
  mountGameSession: vi.fn(),
}));

vi.mock('../ipc/client', () => ({
  setMuted: vi.fn().mockResolvedValue(undefined),
  setPaused: vi.fn().mockResolvedValue(undefined),
  setVolume: vi.fn().mockResolvedValue(undefined),
  toggleFullscreen: vi.fn().mockResolvedValue({ fullscreen: true }),
}));

import { mountGameSession } from '../emu/session';
import { useToastStore } from '../store/toastStore';

import { GameView } from './GameView';

const SAMPLE_INFO: SessionInfo = {
  sha1: 'a'.repeat(40),
  has_battery: false,
  mapper: 0,
};

function renderAt(path: string): ReturnType<typeof render> {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/game" element={<GameView />} />
        <Route path="/library" element={<div data-testid="library-route" />} />
      </Routes>
    </MemoryRouter>,
  );
}

beforeEach(() => {
  vi.mocked(mountGameSession).mockReset();
});

afterEach(() => {
  useToastStore.getState().clear();
});

describe('GameView', () => {
  it('mounts a session for the routed library ROM and stops it on unmount', async () => {
    const stop = vi.fn().mockResolvedValue(undefined);
    const session: GameSession = {
      info: SAMPLE_INFO,
      stop,
      resize: vi.fn(),
      setAspectMode: vi.fn(),
      setBindings: vi.fn(),
    };
    vi.mocked(mountGameSession).mockResolvedValue(session);

    const { unmount } = renderAt('/game?rom=demo-rom-id');

    await waitFor(() => {
      expect(mountGameSession).toHaveBeenCalledTimes(1);
    });
    const firstCall = vi.mocked(mountGameSession).mock.calls[0]?.[0];
    expect(firstCall?.rom).toEqual({ kind: 'library', id: 'demo-rom-id' });
    expect(firstCall?.canvas).toBeInstanceOf(HTMLCanvasElement);

    unmount();
    await waitFor(() => {
      expect(stop).toHaveBeenCalledTimes(1);
    });
  });

  it('accepts a "path" ROM source via the ?path= query parameter', async () => {
    const stop = vi.fn().mockResolvedValue(undefined);
    vi.mocked(mountGameSession).mockResolvedValue({
      info: SAMPLE_INFO,
      stop,
      resize: vi.fn(),
      setAspectMode: vi.fn(),
      setBindings: vi.fn(),
    });

    renderAt('/game?path=' + encodeURIComponent('/tmp/loose.nes'));

    await waitFor(() => {
      expect(mountGameSession).toHaveBeenCalled();
    });
    const args = vi.mocked(mountGameSession).mock.calls[0]?.[0];
    expect(args?.rom).toEqual({ kind: 'path', path: '/tmp/loose.nes' });
  });

  it('redirects to the library and emits a toast when no ROM is in the URL', async () => {
    const { getByTestId } = renderAt('/game');

    await waitFor(() => {
      expect(getByTestId('library-route')).toBeInTheDocument();
    });
    expect(mountGameSession).not.toHaveBeenCalled();
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]?.kind).toBe('error');
  });

  it('falls back to the library and shows a toast when mountGameSession rejects', async () => {
    vi.mocked(mountGameSession).mockRejectedValueOnce({
      code: 'InvalidRom',
      details: 'corrupt',
    });

    const { getByTestId } = renderAt('/game?rom=bad-rom');

    await waitFor(() => {
      expect(getByTestId('library-route')).toBeInTheDocument();
    });
    const toasts = useToastStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]?.kind).toBe('error');
  });

  it('stops a session that resolved after the view unmounted', async () => {
    const stop = vi.fn().mockResolvedValue(undefined);
    let resolveSession: (s: GameSession) => void = () => {
      /* set below */
    };
    vi.mocked(mountGameSession).mockReturnValue(
      new Promise<GameSession>((resolve) => {
        resolveSession = resolve;
      }),
    );

    const { unmount } = renderAt('/game?rom=slow-rom');
    unmount();
    resolveSession({
      info: SAMPLE_INFO,
      stop,
      resize: vi.fn(),
      setAspectMode: vi.fn(),
      setBindings: vi.fn(),
    });

    await waitFor(() => {
      expect(stop).toHaveBeenCalledTimes(1);
    });
  });
});
