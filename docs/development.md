# Development Guide

This guide gets a contributor from a fresh checkout to a running rs-nessie
development build, plus how to run tests and benches.

## Prerequisites

### All platforms

- **Rust**, stable. The repository pins the toolchain via
  [`./rust-toolchain.toml`](../rust-toolchain.toml), so installing
  [`rustup`](https://rustup.rs/) and letting it pick up the pin is enough. The
  declared MSRV is `1.78`.
- **Node.js** `>= 20.x` and **npm**. We recommend using a version manager such
  as [nvm](https://github.com/nvm-sh/nvm), `fnm`, or
  [Volta](https://volta.sh/).
- **Git** with submodule support (none are currently used, but recommended).

### Platform-specific Tauri prerequisites

Tauri 2 wraps a system webview and a native bundler. Each OS needs its own
system packages — these are unchanged from upstream
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/).

**Linux (Debian / Ubuntu)**

```bash
sudo apt update
sudo apt install -y \
  libwebkit2gtk-4.1-dev \
  build-essential \
  curl wget file \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

For RPM-based distros, install the equivalent packages (`webkit2gtk4.1-devel`,
`gcc`, `openssl-devel`, `librsvg2-devel`, …).

**macOS**

```bash
xcode-select --install
```

This installs the Apple SDKs and the build tools rs-nessie needs.

**Windows**

- Install the [Microsoft Visual Studio C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
  with the "Desktop development with C++" workload.
- WebView2 is bundled with Windows 10 21H1+ and Windows 11; on older builds,
  install the Evergreen runtime from Microsoft.

## First-time setup

```bash
git clone https://github.com/<your-fork>/rs-nessie.git
cd rs-nessie
npm --prefix app install
cargo build --workspace
```

The Cargo build pulls every Rust dep including `tauri-build`. The npm install
populates `./app/node_modules` for the frontend toolchain.

## Running locally

The Tauri dev loop builds the Rust binary in debug mode, starts the Vite dev
server, and opens the webview pointed at it. From the workspace root:

```bash
npm --prefix app run tauri dev
```

Or, if you want to iterate on the frontend without rebuilding the Rust shell:

```bash
npm --prefix app run dev
```

This starts only the Vite dev server (no emulation; useful for UI work).

## Workspace layout

```
./
├── Cargo.toml              # virtual workspace
├── crates/
│   ├── nessie-core/        # pure emulator core (CPU, PPU, APU, mappers, cart)
│   └── nessie-runtime/     # engine-agnostic emulation loop + sink traits
├── app/
│   ├── package.json        # frontend deps + scripts
│   ├── src/                # React + TS frontend
│   └── src-tauri/          # Tauri host crate (the binary)
├── scripts/
│   ├── build-installer.sh  # POSIX installer driver
│   └── build-installer.ps1 # Windows installer driver
├── docs/                   # this directory
└── .github/workflows/      # ci.yml and release.yml
```

Key crate boundaries:

- [`./crates/nessie-core/`](../crates/nessie-core/) has **zero Tauri / cpal /
  webview deps**. It can be unit-tested headlessly. Tests and benches live
  alongside it.
- [`./crates/nessie-runtime/`](../crates/nessie-runtime/) owns the emulation
  loop and exposes the `AudioSink` / `FrameSink` traits. It depends on
  `nessie-core` only.
- [`./app/src-tauri/`](../app/src-tauri/) is the Tauri binary. It wires
  `cpal` audio, the typed Tauri channel for frames, the filesystem-backed
  library / settings stores, and the IPC commands consumed by the frontend.
- [`./app/src/`](../app/src/) is the React frontend (routes, components,
  stores, IPC wrappers, WebGL2 renderer, input controller).

## Common commands

Run these from the repository root unless noted otherwise.

### Frontend

```bash
npm --prefix app run dev        # Vite dev server
npm --prefix app run typecheck  # tsc --noEmit
npm --prefix app run lint       # ESLint
npm --prefix app run test       # Vitest unit tests
npm --prefix app run build      # production build (assets only)
```

### Rust workspace

```bash
cargo fmt --all -- --check                              # formatting gate
cargo clippy --workspace --all-targets -- -D warnings   # lint gate
cargo build --workspace
cargo test --workspace                                  # unit + integration tests
```

### Targeted Rust tests

```bash
cargo test -p nessie-core cpu          # 6502 CPU tests
cargo test -p nessie-core ppu          # PPU tests
cargo test -p nessie-core apu          # APU tests
cargo test -p nessie-core cart         # iNES parser + mapper tests
cargo test -p nessie-core --test nestest   # nestest.nes golden trace
cargo test -p nessie-core --test smoke     # full-frame smoke ROM
cargo test -p nessie-runtime
cargo test -p rs-nessie                # Tauri host crate (library/settings/commands)
```

### Benches

The CPU/PPU/APU benches live under
[`./crates/nessie-core/benches/`](../crates/nessie-core/benches/). They use
[criterion](https://docs.rs/criterion).

```bash
cargo bench -p nessie-core --bench frame
```

The CI nightly workflow runs the bench with `--save-baseline ci` and compares
subsequent runs against that baseline. Locally you can do the same:

```bash
cargo bench -p nessie-core --bench frame -- --save-baseline local
# later
cargo bench -p nessie-core --bench frame -- --baseline local
```

### Build a production binary

```bash
npm --prefix app run tauri build
```

For installer-level packaging see [build-release.md](./build-release.md).

## Coding standards

- **Rust**: `rustfmt` defaults from
  [`./rustfmt.toml`](../rustfmt.toml); clippy is configured in
  [`./clippy.toml`](../clippy.toml). `unsafe_code = "deny"` is opted in
  per crate where appropriate, and `clippy::unwrap_used` is warned in
  non-test code.
- **TypeScript**: `strict: true`, ESLint with `@typescript-eslint`, Prettier
  for formatting. See [`./app/eslint.config.mjs`](../app/eslint.config.mjs)
  and [`./app/.prettierrc`](../app/.prettierrc).
- Unit tests sit next to the code they cover (Rust: `#[cfg(test)]` modules
  and `tests/`; TS: colocated `*.test.ts(x)`).

## CI

Every PR runs [`./.github/workflows/ci.yml`](../.github/workflows/ci.yml)
across Ubuntu 22.04, macOS 14, and Windows 2022. It enforces:

1. `npm --prefix app ci`
2. `npm --prefix app run lint`
3. `npm --prefix app run typecheck`
4. `npm --prefix app run test -- --run`
5. `cargo fmt --all -- --check`
6. `cargo clippy --workspace --all-targets -- -D warnings`
7. `cargo test --workspace`

A green CI is required before merge.

## Where to look next

- [Architecture overview](./architecture.md) — components, threading,
  IPC contract, framebuffer/audio pipelines.
- [Design decisions](./design-decisions.md) — why we chose Tauri, in-tree
  emulator core, SHA-1 ROM identity, etc.
- [Contributing](./contributing.md) — branch model, commit style, how to
  add a new mapper.
