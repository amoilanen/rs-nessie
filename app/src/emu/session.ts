// Emulation-session orchestrator for the Game view.
//
// Bundles together the three moving pieces a running emulator needs:
//   1. A [`FrameRenderer`] that uploads each frame's pixels into a WebGL2
//      texture and re-draws the active viewport.
//   2. An [`InputController`] that listens for keyboard events and forwards
//      `setButtonState` IPC calls.
//   3. A Tauri `Channel` that streams the framebuffer payload emitted by the
//      Rust host (see `ChannelFrameSink` in
//      `./app/src-tauri/src/commands/emulator.rs` — each message is a raw
//      `[u64 LE frame_index][245_760 bytes RGBA8]` blob).
//
// The factory function [`mountGameSession`] is the single seam consumed by
// the Game route, so tests can mock the whole thing in one place.

import { Channel } from '@tauri-apps/api/core';

import {
  startSession as ipcStartSession,
  stopSession as ipcStopSession,
} from '../ipc/client';
import type {
  FrameMessage,
  RomSource,
  SessionInfo,
} from '../ipc/types';
import type { PlayerBindings } from '../store/settingsStore';

import { FrameRenderer, FRAME_BYTES, type AspectMode } from './frameRenderer';
import { InputController, type SetButtonStateFn } from './inputController';

/** Handle returned by [`mountGameSession`] — controls a live session. */
export interface GameSession {
  /** Cartridge info returned by the host when the session started. */
  readonly info: SessionInfo;
  /**
   * Stop the emulation, unbind the input listeners, and free GPU resources.
   * Idempotent — calling `stop` twice is safe.
   */
  stop: () => Promise<void>;
  /** Force a redraw using the current viewport size. */
  resize: (viewportW: number, viewportH: number) => void;
  /** Switch between 4:3 and 8:7 PAR output modes. */
  setAspectMode: (mode: AspectMode) => void;
  /** Replace the live key bindings without recreating the controller. */
  setBindings: (bindings: PlayerBindings) => void;
}

/** Arguments accepted by [`mountGameSession`]. */
export interface MountGameSessionArgs {
  /** Canvas backing the WebGL2 context. */
  canvas: HTMLCanvasElement;
  /** ROM to load. */
  rom: RomSource;
  /** Key bindings to install in the input controller. */
  bindings: PlayerBindings;
  /** Event target for the keyboard listeners (defaults to `window`). */
  attachTarget?: EventTarget;
  /** Override for the `setButtonState` IPC call (tests). */
  send?: SetButtonStateFn;
}

/**
 * Decode a single framebuffer message from the channel transport.
 *
 * The Rust runtime sends raw bytes prefixed with the little-endian frame
 * index (see `ChannelFrameSink` in
 * `./app/src-tauri/src/commands/emulator.rs`). Anything shorter than the
 * 8-byte prefix + one frame is treated as a transport glitch and dropped.
 */
function decodeFrame(data: unknown): { frameIndex: bigint; pixels: Uint8Array } | null {
  let bytes: Uint8Array | null = null;
  if (data instanceof ArrayBuffer) {
    bytes = new Uint8Array(data);
  } else if (ArrayBuffer.isView(data)) {
    const view = data as ArrayBufferView;
    bytes = new Uint8Array(view.buffer, view.byteOffset, view.byteLength);
  }
  if (!bytes || bytes.byteLength < 8 + FRAME_BYTES) return null;
  const dv = new DataView(bytes.buffer, bytes.byteOffset, 8);
  const frameIndex = dv.getBigUint64(0, true);
  return {
    frameIndex,
    pixels: bytes.subarray(8, 8 + FRAME_BYTES),
  };
}

/**
 * Wire up renderer + input controller + IPC and return a handle that
 * tears the whole pipeline down on `stop()`.
 *
 * The function is `async` because the IPC `start_session` call returns a
 * `SessionInfo` payload that the caller's HUD needs.
 */
export async function mountGameSession(
  args: MountGameSessionArgs,
): Promise<GameSession> {
  const renderer = new FrameRenderer(args.canvas);
  const input = new InputController(args.bindings, args.send);

  // The channel is `unknown` on the wire because the Rust host sends raw
  // byte payloads; the typed `FrameMessage` shape is reconstructed locally
  // in `decodeFrame`. The cast is required because `client.startSession`
  // declares `Channel<FrameMessage>` for ergonomic call-site typing.
  const channel = new Channel<unknown>();
  channel.onmessage = (data: unknown): void => {
    const frame = decodeFrame(data);
    if (!frame) return;
    renderer.upload(frame.pixels);
  };

  let info: SessionInfo;
  try {
    info = await ipcStartSession(
      args.rom,
      channel as unknown as Channel<FrameMessage>,
    );
  } catch (err) {
    // Release GPU resources before re-throwing so the renderer does not
    // leak when the host refuses to start a session.
    renderer.dispose();
    throw err;
  }

  input.attach(args.attachTarget ?? window);

  let stopped = false;
  const stop = async (): Promise<void> => {
    if (stopped) return;
    stopped = true;
    input.detach();
    try {
      await ipcStopSession();
    } catch (err) {
      // Stopping the host session must always succeed locally even if the
      // IPC call failed — we still want to release GPU resources. The
      // error is recorded via `console.warn` so it shows up in dev tools.
      console.warn('stop_session IPC failed:', err);
    }
    renderer.dispose();
  };

  return {
    info,
    stop,
    resize: (w, h) => renderer.draw(w, h),
    setAspectMode: (mode) => renderer.setAspectMode(mode),
    setBindings: (bindings) => input.updateBindings(bindings),
  };
}
