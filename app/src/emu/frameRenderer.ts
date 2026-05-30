// WebGL2 framebuffer renderer.
//
// Owns a 256×240 RGBA8 texture that the emulator runtime updates one frame at
// a time. The renderer compiles a minimal vertex/fragment shader pair drawing
// a single textured full-screen quad, and uses `gl.viewport(...)` to position
// the quad at the largest integer-multiple of the NES resolution that fits
// the host viewport while preserving either the 4:3 broadcast aspect ratio or
// the 8:7 pixel aspect ratio (FR-15/FR-17, spec §5.3 / §6.2).
//
// All GPU resources (program, texture, VBO, VAO) are allocated once in the
// constructor; per-frame work is limited to a single `texSubImage2D` upload
// and a single `drawArrays` issue — no per-frame allocations.

/** NES horizontal resolution in pixels. */
export const NES_WIDTH = 256;
/** NES vertical resolution in pixels. */
export const NES_HEIGHT = 240;
/** Byte size of a single RGBA8 framebuffer. */
export const FRAME_BYTES = NES_WIDTH * NES_HEIGHT * 4;

/** Aspect-ratio policy for the output rectangle (FR-17). */
export type AspectMode = 'ratio-4-3' | 'pixel-aspect-8-7';

/** 4:3 numeric aspect (default per FR-17). */
const ASPECT_4_3 = 4 / 3;
/** 8:7 PAR applied to the 256×240 source. */
const ASPECT_PAR_8_7 = (NES_WIDTH * 8) / (NES_HEIGHT * 7);

const VERTEX_SHADER_SRC = `#version 300 es
layout(location = 0) in vec2 a_position;
layout(location = 1) in vec2 a_uv;
out vec2 v_uv;
void main() {
  v_uv = a_uv;
  gl_Position = vec4(a_position, 0.0, 1.0);
}`;

const FRAGMENT_SHADER_SRC = `#version 300 es
precision highp float;
in vec2 v_uv;
out vec4 outColor;
uniform sampler2D u_tex;
void main() {
  outColor = texture(u_tex, v_uv);
}`;

/** Output rectangle in pixels (top-left origin). */
export interface FitRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Compute the largest integer-multiple rectangle of the NES framebuffer that
 * fits in a viewport while preserving `aspect`. Integer scaling is computed
 * on the height; the width is then `round(height * aspect)`.
 *
 * Exported so unit tests can validate the geometry without instantiating a
 * GL context.
 */
export function computeFitRect(
  viewportW: number,
  viewportH: number,
  aspect: number,
): FitRect {
  if (viewportW <= 0 || viewportH <= 0 || !Number.isFinite(aspect) || aspect <= 0) {
    return { x: 0, y: 0, w: 0, h: 0 };
  }
  const heightStep = NES_HEIGHT;
  const widthStep = Math.max(1, Math.ceil(NES_HEIGHT * aspect));
  const maxByH = Math.floor(viewportH / heightStep);
  const maxByW = Math.floor(viewportW / widthStep);
  const n = Math.max(1, Math.min(maxByH, maxByW));
  const h = heightStep * n;
  const w = Math.round(h * aspect);
  return {
    x: Math.floor((viewportW - w) / 2),
    y: Math.floor((viewportH - h) / 2),
    w,
    h,
  };
}

/**
 * Single-texture quad renderer.
 *
 * The host owns the lifetime of the underlying `WebGL2RenderingContext`; the
 * caller invokes [`FrameRenderer.dispose`] to release GPU resources when the
 * view unmounts.
 */
export class FrameRenderer {
  private readonly gl: WebGL2RenderingContext;
  private readonly program: WebGLProgram;
  private readonly texture: WebGLTexture;
  private readonly vbo: WebGLBuffer;
  private readonly vao: WebGLVertexArrayObject;
  private aspectMode: AspectMode = 'ratio-4-3';

  /**
   * @param canvas Canvas whose `webgl2` context will be used. Ignored when
   * an explicit `context` is supplied (tests pass a mock context).
   * @param context Optional explicit `WebGL2RenderingContext`. Provided so
   * unit tests can drive a mocked GL surface without touching the DOM.
   */
  constructor(canvas: HTMLCanvasElement, context?: WebGL2RenderingContext) {
    const gl = context ?? canvas.getContext('webgl2');
    if (!gl) {
      throw new Error('WebGL2 is not supported in this environment.');
    }
    this.gl = gl;
    this.program = this.buildProgram();
    this.texture = this.createTexture();
    const quad = this.createQuad();
    this.vbo = quad.vbo;
    this.vao = quad.vao;

    // Bind the texture to texture-unit 0 once. `u_tex` defaults to 0 so we
    // do not actually need to set it, but doing so keeps the contract
    // explicit and exercises the uniform location lookup in tests.
    const loc = gl.getUniformLocation(this.program, 'u_tex');
    if (loc !== null) {
      gl.useProgram(this.program);
      gl.uniform1i(loc, 0);
    }
  }

  /** Switch between 4:3 broadcast and 8:7 pixel-aspect output modes. */
  setAspectMode(mode: AspectMode): void {
    this.aspectMode = mode;
  }

  /** Return the active aspect-ratio policy. */
  getAspectMode(): AspectMode {
    return this.aspectMode;
  }

  /**
   * Upload one frame's pixels into the pre-allocated texture.
   *
   * `pixels` must be at least `FRAME_BYTES` bytes long. Shorter buffers are
   * silently dropped to absorb the occasional transport hiccup without
   * crashing the view (frames arriving at 60 Hz mean a dropped frame is
   * indistinguishable to the user).
   */
  upload(pixels: ArrayBuffer | ArrayBufferView): void {
    const gl = this.gl;
    let view: Uint8Array;
    if (pixels instanceof ArrayBuffer) {
      view = new Uint8Array(pixels);
    } else {
      view = new Uint8Array(
        pixels.buffer,
        pixels.byteOffset,
        pixels.byteLength,
      );
    }
    if (view.byteLength < FRAME_BYTES) return;
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.texSubImage2D(
      gl.TEXTURE_2D,
      0,
      0,
      0,
      NES_WIDTH,
      NES_HEIGHT,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      view.subarray(0, FRAME_BYTES),
    );
  }

  /**
   * Draw the most recently uploaded frame into a `viewportW × viewportH`
   * surface. The output rectangle is the largest integer multiple of the
   * NES height that fits the viewport while preserving the active aspect
   * mode; areas outside the rectangle are cleared to opaque black.
   */
  draw(viewportW: number, viewportH: number): void {
    const gl = this.gl;
    // Clear the whole viewport (letterbox background).
    gl.viewport(0, 0, viewportW, viewportH);
    gl.clearColor(0, 0, 0, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);

    const aspect =
      this.aspectMode === 'ratio-4-3' ? ASPECT_4_3 : ASPECT_PAR_8_7;
    const rect = computeFitRect(viewportW, viewportH, aspect);
    if (rect.w <= 0 || rect.h <= 0) return;

    gl.viewport(rect.x, rect.y, rect.w, rect.h);
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.bindTexture(gl.TEXTURE_2D, this.texture);
    gl.drawArrays(gl.TRIANGLES, 0, 6);
    gl.bindVertexArray(null);
  }

  /** Release all GPU resources owned by the renderer. */
  dispose(): void {
    const gl = this.gl;
    gl.deleteBuffer(this.vbo);
    gl.deleteVertexArray(this.vao);
    gl.deleteTexture(this.texture);
    gl.deleteProgram(this.program);
  }

  // -----------------------------------------------------------------------
  // Internals
  // -----------------------------------------------------------------------

  private compileShader(type: number, source: string): WebGLShader {
    const gl = this.gl;
    const shader = gl.createShader(type);
    if (!shader) throw new Error('Failed to create WebGL shader.');
    gl.shaderSource(shader, source);
    gl.compileShader(shader);
    if (!gl.getShaderParameter(shader, gl.COMPILE_STATUS)) {
      const info = gl.getShaderInfoLog(shader) ?? '<no info>';
      gl.deleteShader(shader);
      throw new Error(`Failed to compile shader: ${info}`);
    }
    return shader;
  }

  private buildProgram(): WebGLProgram {
    const gl = this.gl;
    const vs = this.compileShader(gl.VERTEX_SHADER, VERTEX_SHADER_SRC);
    const fs = this.compileShader(gl.FRAGMENT_SHADER, FRAGMENT_SHADER_SRC);
    const program = gl.createProgram();
    if (!program) throw new Error('Failed to create WebGL program.');
    gl.attachShader(program, vs);
    gl.attachShader(program, fs);
    gl.linkProgram(program);
    if (!gl.getProgramParameter(program, gl.LINK_STATUS)) {
      const info = gl.getProgramInfoLog(program) ?? '<no info>';
      gl.deleteProgram(program);
      throw new Error(`Failed to link program: ${info}`);
    }
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    return program;
  }

  private createTexture(): WebGLTexture {
    const gl = this.gl;
    const texture = gl.createTexture();
    if (!texture) throw new Error('Failed to allocate WebGL texture.');
    gl.bindTexture(gl.TEXTURE_2D, texture);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE);
    gl.texImage2D(
      gl.TEXTURE_2D,
      0,
      gl.RGBA8,
      NES_WIDTH,
      NES_HEIGHT,
      0,
      gl.RGBA,
      gl.UNSIGNED_BYTE,
      null,
    );
    return texture;
  }

  private createQuad(): { vbo: WebGLBuffer; vao: WebGLVertexArrayObject } {
    const gl = this.gl;
    // Six vertices forming two triangles covering the full NDC quad.
    // Each vertex is (x, y, u, v). The texture's V axis is flipped relative
    // to the NES framebuffer so the first pixel of the upload (top-left of
    // the screen) renders at the top of the viewport.
    const data = new Float32Array([
      -1, -1, 0, 1,
       1, -1, 1, 1,
      -1,  1, 0, 0,
      -1,  1, 0, 0,
       1, -1, 1, 1,
       1,  1, 1, 0,
    ]);
    const vao = gl.createVertexArray();
    if (!vao) throw new Error('Failed to allocate vertex array object.');
    gl.bindVertexArray(vao);
    const vbo = gl.createBuffer();
    if (!vbo) throw new Error('Failed to allocate vertex buffer.');
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 16, 0);
    gl.enableVertexAttribArray(1);
    gl.vertexAttribPointer(1, 2, gl.FLOAT, false, 16, 8);
    gl.bindVertexArray(null);
    return { vbo, vao };
  }
}
