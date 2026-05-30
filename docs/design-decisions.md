# Design Decisions

This document is derived from spec §7. Each entry follows the
"Context / Decision / Consequences" format and serves as a short architectural
log of non-obvious choices made for the initial release.

For the full technical specification, see
[`./.zenflow/tasks/create-a-nes-8bit-emulator-which-2e40/spec.md`](../.zenflow/tasks/create-a-nes-8bit-emulator-which-2e40/spec.md).

---

## 1. In-tree emulator core (no third-party NES crate)

### Context

There are several existing Rust NES emulator crates on crates.io. Reusing one
would in principle save us weeks of work on CPU/PPU/APU correctness.

### Decision

Write the emulator core in-tree under [`./crates/nessie-core/`](../crates/nessie-core/).

### Consequences

- **Pros**: full control over the API surface our runtime depends on; no
  upstream churn or licensing surprises; we can hit the FR-2 mapper bar
  (mappers 0/1/2/3/4) with focused tests and a known `nestest` golden trace;
  unit testing and benching are unblocked because we own the boundaries.
- **Cons**: more upfront work to bring CPU + PPU + APU + the five mappers up
  to a playable bar; we cannot piggyback on someone else's `nestest`
  certification.
- **Mitigations**: the [nestest](https://wiki.nesdev.org/w/index.php/Emulator_tests)
  ROM and golden log are committed under
  [`./crates/nessie-core/tests/fixtures/`](../crates/nessie-core/tests/fixtures/)
  and run as a `cargo test` integration test on every CI run, on every OS.

---

## 2. Tauri 2 over Electron or pure-Rust UI (`egui`)

### Context

The task description explicitly suggests Tauri. We considered the alternatives:

- **Electron** — bundles Chromium, ships ~150 MB installers, runs a Node
  runtime alongside the renderer.
- **`egui` / `iced`** — pure-Rust immediate-mode GUI, no webview.

### Decision

Use Tauri 2 with a React + TypeScript webview frontend.

### Consequences

- **Pros**: matches the task's explicit suggestion; produces small native
  installers (NFR-6) because Tauri uses each OS's built-in webview; lets us
  build the library / settings UI in standard web tech without sacrificing a
  Rust emulator core; first-class support for the four installer formats
  required by FR-28 via `tauri-bundler`; stable IPC `Channel` API suitable for
  streaming framebuffers from Rust to the webview.
- **Cons**: webview behavior varies slightly across OSes (WebKit on macOS/Linux,
  WebView2 on Windows); we have to think about CSP and the IPC allowlist.
- **Mitigations**: CSP is locked to `default-src 'self'` (no remote origins).
  The `tauri-plugin-dialog` is the only allowlisted plugin; all other system
  access is via explicit commands.

---

## 3. Single-window render via WebGL2 + framebuffer channel

### Context

We could open a separate native game window via `winit` + `pixels` for
rendering, leaving the webview only for the library and settings UI. That
would let us render with native swapchain control.

### Decision

Render the NES framebuffer in the same Tauri webview using a `<canvas>` and
WebGL2 `texSubImage2D` into a pre-allocated `RGBA8` 256×240 texture. Frames
flow over a typed Tauri `Channel<FrameMessage>` from the emulation thread.

### Consequences

- **Pros**: one window, no platform-specific window-parenting issues, no
  second event loop to coordinate; the UI seamlessly overlays a HUD on top of
  the canvas; the `Channel` API is back-pressured by Tauri internally.
- **Cons**: per-frame bandwidth is ~240 KB × 60 Hz = ~14 MB/s, going through
  IPC; webview compositors can add a frame of latency.
- **Mitigations**: the texture and unit-quad VBO are allocated once at
  session start; per-frame uploads are zero-allocation in the renderer; the
  framebuffer is sent as a raw `ArrayBuffer` (via `bytemuck::cast_slice`), not
  JSON, which keeps IPC cost a single memcpy. Throughput has been benched to
  comfortably exceed 60 Hz on commodity hardware.

---

## 4. Audio is NOT routed through IPC

### Context

Symmetry suggests audio could also flow through a Tauri `Channel` to the
webview's Web Audio API. In practice that route adds jitter, OS-specific
audio session quirks, and Web Audio's own scheduling rules.

### Decision

Run `cpal` in-process on the Rust side. The emulation thread pushes mono
`f32` samples into a lock-free SPSC ring buffer; the cpal callback on the
audio thread pulls from it directly. If the ring is starved, cpal outputs
silence rather than blocking.

### Consequences

- **Pros**: tight, OS-native audio latency (NFR-1); the audio device never
  stalls because the callback is non-blocking; no webview-related glitch class
  to debug.
- **Cons**: introduces a `cpal` dependency in the Tauri host; the audio
  resampler lives on the Rust side (NES native sample rate ≈ 1.789 MHz CPU
  ticks downsampled to 48 kHz / device rate).
- **Notes**: the initial resampler is a simple naive low-pass + decimate.
  Higher-quality resampling (e.g. polyphase FIR) is a known follow-up; see
  the audio module for the precise algorithm and trade-offs.

---

## 5. JSON store via `tauri-plugin-store` (not SQLite)

### Context

A persistent ROM library and collections need durable storage. We considered:

- **SQLite** via `rusqlite` — battle-tested, but adds a native dependency and
  increases the installer size.
- **JSON files** managed by Rust directly.

### Decision

Store the library and settings as plain JSON files under
`<config>/dev.rs-nessie/` (atomic write via `<tmp>` + `rename`). Use
`tauri-plugin-store` where convenient for typed access.

### Consequences

- **Pros**: data set is small (low thousands of ROMs at most) and shallow;
  zero native dependencies; the file is trivially inspectable and backup-able;
  installer stays small (NFR-6); CI builds are faster.
- **Cons**: no transactions; large-write throughput is bounded by the
  rewrite-and-rename cost.
- **Mitigations**: writes are infrequent (only on user actions) and the
  library is small. If the data set ever grows beyond a few MB, migrating to
  SQLite is straightforward — the JSON schema is versioned via a top-level
  `version: u32` field.

---

## 6. ROM identity = SHA-1 of file contents

### Context

We need a stable identifier for each ROM that:

1. Lets battery-backed save files survive the user moving the `.nes` file on
   disk (assumption A-4, FR-14).
2. Dedupes imports of the same content from different paths.
3. Is cheap to compute on import.

### Decision

Use the SHA-1 of the file contents. Save files are named `<sha1>.srm`. On
import, if a `RomEntry` with the same SHA-1 already exists, update its `path`
and return the existing entry instead of creating a duplicate.

### Consequences

- **Pros**: stable across file moves and renames; cheap to compute (SHA-1 on
  a few-MB ROM is sub-millisecond); not a security boundary, so SHA-1's
  collision-resistance flaws are irrelevant here.
- **Cons**: SHA-1 is not collision-resistant in adversarial settings. For a
  local ROM library this is a non-issue (the worst case is a malicious user
  intentionally colliding two ROMs, which would only affect their own
  library).
- **Alternatives considered**: BLAKE3 would be faster; CRC32 would be cheaper
  still; both would work. SHA-1 was chosen because it is also a standard
  identifier used by other emulator communities, making it easier to
  cross-reference databases later.

---

## 7. Default to NTSC; PAL deferred

### Context

NES variants exist in NTSC and PAL flavors with different timings and frame
rates (60 Hz vs 50 Hz). Supporting both means parameterizing CPU/PPU/APU
clocking and adding region detection.

### Decision

Default to NTSC for the initial release (FR-4, A-3). Structure the core so
PPU/APU can be parameterized later without API breakage.

### Consequences

- **Pros**: simpler initial implementation; matches the acceptance bar (FR-2
  reference titles are all NTSC); avoids the regression risk of region
  detection in iNES headers.
- **Cons**: PAL ROMs may run, but at the wrong refresh rate and with mistuned
  audio.
- **Mitigations**: the `Nes` API takes no region parameter today; when PAL
  support is added, it will go through a `Region` enum and a builder method
  on `Nes` so existing callers compile unchanged.
