// Unit tests for `FrameRenderer`.
//
// jsdom does not implement WebGL2, so each test constructs a hand-rolled mock
// `WebGL2RenderingContext` exposing only the symbols the renderer touches.
// Assertions focus on the call shape (texture created once, `texSubImage2D`
// per upload, `drawArrays` per draw) rather than rendered pixels.

import { describe, expect, it, vi } from 'vitest';

import {
  computeFitRect,
  FRAME_BYTES,
  FrameRenderer,
  NES_HEIGHT,
  NES_WIDTH,
} from './frameRenderer';

/**
 * Minimal mock of the subset of `WebGL2RenderingContext` used by
 * [`FrameRenderer`]. Each method is a `vi.fn()` so tests can assert call
 * shape. Constants use the canonical WebGL numeric values where it matters
 * (most do not — the renderer just passes them through to the mock).
 */
function createGlMock(): Record<string, ReturnType<typeof vi.fn> | number> {
  return {
    // Tokens (numeric values are not asserted by the production code; the
    // renderer simply passes whatever it reads from the context back into
    // its own calls. We still use real values for self-documentation).
    VERTEX_SHADER: 0x8b31,
    FRAGMENT_SHADER: 0x8b30,
    COMPILE_STATUS: 0x8b81,
    LINK_STATUS: 0x8b82,
    TEXTURE_2D: 0x0de1,
    TEXTURE_MIN_FILTER: 0x2801,
    TEXTURE_MAG_FILTER: 0x2800,
    TEXTURE_WRAP_S: 0x2802,
    TEXTURE_WRAP_T: 0x2803,
    NEAREST: 0x2600,
    CLAMP_TO_EDGE: 0x812f,
    RGBA8: 0x8058,
    RGBA: 0x1908,
    UNSIGNED_BYTE: 0x1401,
    ARRAY_BUFFER: 0x8892,
    STATIC_DRAW: 0x88e4,
    FLOAT: 0x1406,
    TRIANGLES: 0x0004,
    COLOR_BUFFER_BIT: 0x4000,

    // Shaders / programs
    createShader: vi.fn(() => ({ kind: 'shader' })),
    shaderSource: vi.fn(),
    compileShader: vi.fn(),
    getShaderParameter: vi.fn(() => true),
    getShaderInfoLog: vi.fn(() => ''),
    deleteShader: vi.fn(),

    createProgram: vi.fn(() => ({ kind: 'program' })),
    attachShader: vi.fn(),
    linkProgram: vi.fn(),
    getProgramParameter: vi.fn(() => true),
    getProgramInfoLog: vi.fn(() => ''),
    useProgram: vi.fn(),
    deleteProgram: vi.fn(),
    getUniformLocation: vi.fn(() => ({ kind: 'uniform' })),
    uniform1i: vi.fn(),

    // Textures
    createTexture: vi.fn(() => ({ kind: 'texture' })),
    bindTexture: vi.fn(),
    texParameteri: vi.fn(),
    texImage2D: vi.fn(),
    texSubImage2D: vi.fn(),
    deleteTexture: vi.fn(),

    // Buffers
    createBuffer: vi.fn(() => ({ kind: 'buffer' })),
    bindBuffer: vi.fn(),
    bufferData: vi.fn(),
    deleteBuffer: vi.fn(),

    // VAO
    createVertexArray: vi.fn(() => ({ kind: 'vao' })),
    bindVertexArray: vi.fn(),
    deleteVertexArray: vi.fn(),

    // Attributes
    enableVertexAttribArray: vi.fn(),
    vertexAttribPointer: vi.fn(),

    // Draw state
    viewport: vi.fn(),
    clearColor: vi.fn(),
    clear: vi.fn(),
    drawArrays: vi.fn(),
  };
}

function makeRenderer(): {
  renderer: FrameRenderer;
  gl: ReturnType<typeof createGlMock>;
} {
  const canvas = document.createElement('canvas');
  const gl = createGlMock();
  const renderer = new FrameRenderer(
    canvas,
    gl as unknown as WebGL2RenderingContext,
  );
  return { renderer, gl };
}

describe('FrameRenderer', () => {
  it('allocates exactly one texture during construction', () => {
    const { gl } = makeRenderer();
    expect((gl.createTexture as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
    expect((gl.texImage2D as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
    const texImage2D = gl.texImage2D as ReturnType<typeof vi.fn>;
    const args = texImage2D.mock.calls[0];
    // Canonical: gl.texImage2D(TEXTURE_2D, level, RGBA8, w, h, 0, RGBA, UNSIGNED_BYTE, data)
    expect(args?.[3]).toBe(NES_WIDTH);
    expect(args?.[4]).toBe(NES_HEIGHT);
  });

  it('calls texSubImage2D on every upload and does not re-allocate the texture', () => {
    const { renderer, gl } = makeRenderer();
    const buf = new ArrayBuffer(FRAME_BYTES);
    renderer.upload(buf);
    renderer.upload(buf);
    renderer.upload(buf);
    expect((gl.texSubImage2D as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(3);
    // texImage2D stays at one (initial allocation).
    expect((gl.texImage2D as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
  });

  it('silently ignores short uploads', () => {
    const { renderer, gl } = makeRenderer();
    renderer.upload(new ArrayBuffer(16));
    expect((gl.texSubImage2D as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(0);
  });

  it('issues a single drawArrays per draw and centers the rectangle', () => {
    const { renderer, gl } = makeRenderer();
    renderer.draw(1024, 768);
    expect((gl.drawArrays as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
    // Two viewport calls: the letterbox clear + the integer-scaled draw.
    const viewportCalls = (gl.viewport as ReturnType<typeof vi.fn>).mock.calls;
    expect(viewportCalls).toHaveLength(2);
    expect(viewportCalls[0]).toEqual([0, 0, 1024, 768]);
  });

  it('disposes all GL resources', () => {
    const { renderer, gl } = makeRenderer();
    renderer.dispose();
    expect((gl.deleteBuffer as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
    expect((gl.deleteVertexArray as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
    expect((gl.deleteTexture as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
    expect((gl.deleteProgram as ReturnType<typeof vi.fn>).mock.calls).toHaveLength(1);
  });

  it('toggles aspect modes', () => {
    const { renderer } = makeRenderer();
    expect(renderer.getAspectMode()).toBe('ratio-4-3');
    renderer.setAspectMode('pixel-aspect-8-7');
    expect(renderer.getAspectMode()).toBe('pixel-aspect-8-7');
  });
});

describe('computeFitRect', () => {
  it('returns the largest integer-multiple of 240 that fits the viewport (4:3)', () => {
    // 480 / 240 = 2 → height = 480, width = 640 (4:3), centered.
    const rect = computeFitRect(640, 480, 4 / 3);
    expect(rect.h).toBe(480);
    expect(rect.w).toBe(640);
    expect(rect.x).toBe(0);
    expect(rect.y).toBe(0);
  });

  it('letterboxes when the viewport is wider than 4:3', () => {
    const rect = computeFitRect(1920, 480, 4 / 3);
    expect(rect.h).toBe(480);
    expect(rect.w).toBe(640);
    expect(rect.x).toBe((1920 - 640) / 2);
    expect(rect.y).toBe(0);
  });

  it('pillarboxes when the viewport is taller than 4:3', () => {
    const rect = computeFitRect(640, 960, 4 / 3);
    expect(rect.h).toBe(480);
    expect(rect.w).toBe(640);
    expect(rect.y).toBe((960 - 480) / 2);
  });

  it('returns at least the 1× rectangle even in a tiny viewport', () => {
    const rect = computeFitRect(10, 10, 4 / 3);
    expect(rect.h).toBe(NES_HEIGHT);
    // Width is rounded from height * aspect.
    expect(rect.w).toBe(Math.round(NES_HEIGHT * (4 / 3)));
  });

  it('returns an empty rect for non-positive inputs', () => {
    expect(computeFitRect(0, 100, 1.33)).toEqual({ x: 0, y: 0, w: 0, h: 0 });
    expect(computeFitRect(100, 0, 1.33)).toEqual({ x: 0, y: 0, w: 0, h: 0 });
    expect(computeFitRect(100, 100, 0)).toEqual({ x: 0, y: 0, w: 0, h: 0 });
  });
});
