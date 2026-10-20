// Unit tests for the Game-view session orchestrator.
//
// WebGL and Tauri IPC are mocked: the tests drive the frame channel by hand
// and assert that every streamed frame is uploaded *and presented* — a
// regression test for frames being uploaded to the texture but never drawn.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', async () => {
  const shim = await import('../ipc/tauri-shim');
  return shim.tauriCoreMockFactory();
});

const rendererMocks = vi.hoisted(() => ({
  upload: vi.fn(),
  draw: vi.fn(),
  dispose: vi.fn(),
  setAspectMode: vi.fn(),
}));

vi.mock('./frameRenderer', async (importOriginal) => {
  const actual = await importOriginal<typeof FrameRendererModule>();
  class FakeRenderer {
    upload = rendererMocks.upload;
    draw = rendererMocks.draw;
    dispose = rendererMocks.dispose;
    setAspectMode = rendererMocks.setAspectMode;
  }
  return { ...actual, FrameRenderer: FakeRenderer };
});

vi.mock('../ipc/client', () => ({
  startSession: vi.fn(),
  stopSession: vi.fn().mockResolvedValue(undefined),
  setButtonState: vi.fn().mockResolvedValue(undefined),
}));

import type { MockChannel } from '../ipc/tauri-shim';
import type * as FrameRendererModule from './frameRenderer';
import { startSession, stopSession } from '../ipc/client';
import type { PlayerBindings } from '../store/settingsStore';
import { DEFAULT_BINDINGS } from '../store/settingsStore';

import { FRAME_BYTES } from './frameRenderer';
import { mountGameSession } from './session';

const INFO = { sha1: 'b'.repeat(40), has_battery: false, mapper: 0, session_id: 42 };

/** Build a `[u64 LE frame_index][RGBA8 frame]` payload like the Rust host. */
function framePayload(index: number, fill: number): Uint8Array {
  const bytes = new Uint8Array(8 + FRAME_BYTES).fill(fill);
  new DataView(bytes.buffer).setBigUint64(0, BigInt(index), true);
  return bytes;
}

/** Mount a session with a synchronous redraw scheduler. */
async function mount(scheduled: Array<() => void>): Promise<{
  channel: MockChannel;
  stop: () => Promise<void>;
  resize: (w: number, h: number) => void;
}> {
  const canvas = document.createElement('canvas');
  canvas.width = 512;
  canvas.height = 480;
  const session = await mountGameSession({
    canvas,
    rom: { kind: 'library', id: 'rom-1' },
    bindings: DEFAULT_BINDINGS as PlayerBindings,
    attachTarget: new EventTarget(),
    send: vi.fn().mockResolvedValue(undefined),
    scheduleFrame: (cb) => scheduled.push(cb),
  });
  const call = vi.mocked(startSession).mock.calls[0];
  if (!call) throw new Error('startSession was not called');
  const channel = call[1] as unknown as MockChannel;
  return { channel, stop: session.stop, resize: session.resize };
}

beforeEach(() => {
  vi.mocked(startSession).mockReset().mockResolvedValue(INFO);
  vi.mocked(stopSession).mockClear();
  Object.values(rendererMocks).forEach((m) => m.mockClear());
});

afterEach(() => {
  vi.clearAllMocks();
});

describe('mountGameSession', () => {
  it('uploads and draws every frame streamed through the channel', async () => {
    const scheduled: Array<() => void> = [];
    const { channel } = await mount(scheduled);

    channel.onmessage?.(framePayload(0, 0x11));
    expect(rendererMocks.upload).toHaveBeenCalledTimes(1);
    const pixels = rendererMocks.upload.mock.calls[0]?.[0] as Uint8Array;
    expect(pixels.byteLength).toBe(FRAME_BYTES);
    expect(pixels[0]).toBe(0x11);

    // The draw happens on the next display refresh, at the canvas size.
    expect(scheduled).toHaveLength(1);
    scheduled.shift()?.();
    expect(rendererMocks.draw).toHaveBeenCalledWith(512, 480);

    // A second frame triggers a second draw.
    channel.onmessage?.(framePayload(1, 0x22));
    scheduled.shift()?.();
    expect(rendererMocks.upload).toHaveBeenCalledTimes(2);
    expect(rendererMocks.draw).toHaveBeenCalledTimes(2);
  });

  it('coalesces bursts of frames into a single draw per refresh', async () => {
    const scheduled: Array<() => void> = [];
    const { channel } = await mount(scheduled);

    channel.onmessage?.(framePayload(0, 1));
    channel.onmessage?.(framePayload(1, 2));
    channel.onmessage?.(framePayload(2, 3));
    expect(rendererMocks.upload).toHaveBeenCalledTimes(3);
    expect(scheduled).toHaveLength(1);
    scheduled.shift()?.();
    expect(rendererMocks.draw).toHaveBeenCalledTimes(1);
  });

  it('draws at the most recent viewport size after a resize', async () => {
    const scheduled: Array<() => void> = [];
    const { channel, resize } = await mount(scheduled);

    resize(1024, 960);
    rendererMocks.draw.mockClear();
    channel.onmessage?.(framePayload(0, 1));
    scheduled.shift()?.();
    expect(rendererMocks.draw).toHaveBeenCalledWith(1024, 960);
  });

  it('ignores frames after stop and does not draw into a disposed renderer', async () => {
    const scheduled: Array<() => void> = [];
    const { channel, stop } = await mount(scheduled);

    channel.onmessage?.(framePayload(0, 1));
    await stop();
    expect(stopSession).toHaveBeenCalledTimes(1);
    // The stop addresses this session only (see the stale-stop race).
    expect(stopSession).toHaveBeenCalledWith(42);
    expect(rendererMocks.dispose).toHaveBeenCalledTimes(1);

    // A redraw queued before stop must not touch the disposed renderer.
    scheduled.shift()?.();
    channel.onmessage?.(framePayload(1, 1));
    expect(rendererMocks.draw).not.toHaveBeenCalled();
    expect(rendererMocks.upload).toHaveBeenCalledTimes(1);
  });

  it('drops truncated payloads', async () => {
    const scheduled: Array<() => void> = [];
    const { channel } = await mount(scheduled);

    channel.onmessage?.(new Uint8Array(16));
    expect(rendererMocks.upload).not.toHaveBeenCalled();
    expect(scheduled).toHaveLength(0);
  });
});
