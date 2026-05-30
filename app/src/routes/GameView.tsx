// Game route — active emulation surface.
//
// The view owns three lifetimes:
//   1. A `<canvas>` element backing a WebGL2 context (the renderer).
//   2. An [`InputController`] that captures keyboard events for P1 and P2.
//   3. The host emulator session, opened via [`mountGameSession`] which
//      bundles the renderer / controller / IPC channel.
//
// Mounting starts a session for the ROM identified in the URL query string
// (`?rom=<id>` for a library entry, `?path=<absolute>` for a loose `.nes`
// file). Unmounting tears the session down idempotently.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
} from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';

import { VolumeSlider } from '../components/VolumeSlider';
import {
  setMuted as ipcSetMuted,
  setPaused as ipcSetPaused,
  setVolume as ipcSetVolume,
  toggleFullscreen as ipcToggleFullscreen,
} from '../ipc/client';
import { formatAppError } from '../ipc/format';
import type { RomSource, SessionInfo } from '../ipc/types';
import {
  DEFAULT_BINDINGS,
  DEFAULT_SETTINGS,
  useSettingsStore,
} from '../store/settingsStore';
import { pushErrorToast } from '../store/toastStore';

import { mountGameSession, type GameSession } from '../emu/session';

/** Parse the routed `?rom=` / `?path=` query into a `RomSource`. */
function parseRomSource(params: URLSearchParams): RomSource | null {
  const id = params.get('rom');
  if (id) return { kind: 'library', id };
  const path = params.get('path');
  if (path) return { kind: 'path', path };
  return null;
}

export function GameView(): ReactElement {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const rom = useMemo(() => parseRomSource(searchParams), [searchParams]);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const sessionRef = useRef<GameSession | null>(null);

  const bindings = useSettingsStore(
    (s) => s.settings?.bindings ?? DEFAULT_BINDINGS,
  );
  const fullscreenShortcut = useSettingsStore(
    (s) => s.settings?.fullscreen_shortcut ?? DEFAULT_SETTINGS.fullscreen_shortcut,
  );
  const settingsVolume = useSettingsStore(
    (s) => s.settings?.volume ?? DEFAULT_SETTINGS.volume,
  );
  const settingsMuted = useSettingsStore(
    (s) => s.settings?.muted ?? DEFAULT_SETTINGS.muted,
  );

  const [paused, setPausedLocal] = useState(false);
  const [muted, setMutedLocal] = useState(settingsMuted);
  const [volume, setVolumeLocal] = useState(settingsVolume);
  const [sessionInfo, setSessionInfo] = useState<SessionInfo | null>(null);
  const [hudVisible, setHudVisible] = useState(true);

  // Reflect persisted volume / mute changes into the HUD.
  useEffect(() => {
    setMutedLocal(settingsMuted);
  }, [settingsMuted]);
  useEffect(() => {
    setVolumeLocal(settingsVolume);
  }, [settingsVolume]);

  // Start (and tear down) the session whenever the routed ROM changes.
  useEffect(() => {
    if (!rom) {
      pushErrorToast('No ROM specified. Returning to the library.');
      navigate('/library', { replace: true });
      return;
    }
    const canvas = canvasRef.current;
    if (!canvas) return;

    let cancelled = false;
    let handle: GameSession | null = null;

    mountGameSession({ canvas, rom, bindings })
      .then((session) => {
        if (cancelled) {
          // The view unmounted before the IPC resolved: stop immediately so
          // the host does not run a session with no consumer.
          void session.stop();
          return;
        }
        handle = session;
        sessionRef.current = session;
        setSessionInfo(session.info);
        // Initial draw using the current canvas size.
        const dpr = window.devicePixelRatio || 1;
        const w = Math.max(1, Math.floor(canvas.clientWidth * dpr));
        const h = Math.max(1, Math.floor(canvas.clientHeight * dpr));
        canvas.width = w;
        canvas.height = h;
        session.resize(w, h);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        pushErrorToast(formatAppError(err));
        navigate('/library', { replace: true });
      });

    return (): void => {
      cancelled = true;
      const local = handle ?? sessionRef.current;
      sessionRef.current = null;
      if (local) void local.stop();
    };
    // `bindings` deliberately excluded — bindings updates flow through the
    // `setBindings` effect below so we do not tear the whole session down on
    // every key remap.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rom, navigate]);

  // Live-update the controller's bindings.
  useEffect(() => {
    sessionRef.current?.setBindings(bindings);
  }, [bindings]);

  // Resize observer: redraw the canvas at the current pixel size when the
  // host viewport changes (including fullscreen toggles).
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    const apply = (): void => {
      const session = sessionRef.current;
      if (!session) return;
      const dpr = window.devicePixelRatio || 1;
      const w = Math.max(1, Math.floor(canvas.clientWidth * dpr));
      const h = Math.max(1, Math.floor(canvas.clientHeight * dpr));
      canvas.width = w;
      canvas.height = h;
      session.resize(w, h);
    };
    const observer =
      typeof ResizeObserver !== 'undefined' ? new ResizeObserver(apply) : null;
    observer?.observe(canvas);
    window.addEventListener('resize', apply);
    return (): void => {
      observer?.disconnect();
      window.removeEventListener('resize', apply);
    };
  }, []);

  // Fullscreen shortcut handling (FR-16). The default is `F11`; `Escape`
  // always exits fullscreen as a convenience.
  useEffect(() => {
    const handler = (event: KeyboardEvent): void => {
      const isFullscreenKey =
        event.code === fullscreenShortcut || event.key === fullscreenShortcut;
      if (isFullscreenKey) {
        event.preventDefault();
        ipcToggleFullscreen().catch((err) =>
          pushErrorToast(formatAppError(err)),
        );
        return;
      }
      if (event.code === 'Escape') {
        ipcToggleFullscreen().catch(() => {
          /* best-effort */
        });
      }
    };
    window.addEventListener('keydown', handler);
    return (): void => {
      window.removeEventListener('keydown', handler);
    };
  }, [fullscreenShortcut]);

  const handleTogglePause = useCallback((): void => {
    const next = !paused;
    setPausedLocal(next);
    ipcSetPaused(next).catch((err) => pushErrorToast(formatAppError(err)));
  }, [paused]);

  const handleVolume = useCallback((next: number): void => {
    setVolumeLocal(next);
    ipcSetVolume(next).catch((err) => pushErrorToast(formatAppError(err)));
  }, []);

  const handleMuted = useCallback((next: boolean): void => {
    setMutedLocal(next);
    ipcSetMuted(next).catch((err) => pushErrorToast(formatAppError(err)));
  }, []);

  const handleBackToLibrary = useCallback((): void => {
    navigate('/library');
  }, [navigate]);

  return (
    <section
      className="game-view"
      aria-labelledby="game-heading"
      onMouseMove={(): void => setHudVisible(true)}
    >
      <h1 id="game-heading" className="visually-hidden">
        Game
      </h1>
      <canvas
        ref={canvasRef}
        className="game-view__canvas"
        aria-label="Emulator output"
      />
      {hudVisible ? (
        <div className="game-view__hud" role="toolbar" aria-label="Emulator controls">
          <button
            type="button"
            className="button button--small"
            onClick={handleBackToLibrary}
          >
            Back to library
          </button>
          <button
            type="button"
            className="button button--small"
            onClick={handleTogglePause}
            aria-pressed={paused}
          >
            {paused ? 'Resume' : 'Pause'}
          </button>
          <VolumeSlider
            volume={volume}
            muted={muted}
            onVolumeChange={handleVolume}
            onMutedChange={handleMuted}
          />
          {sessionInfo ? (
            <span
              className="game-view__info"
              aria-label="Cartridge information"
            >
              Mapper {sessionInfo.mapper}
              {sessionInfo.has_battery ? ' · Battery' : ''}
            </span>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
