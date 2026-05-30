# rs-nessie

**rs-nessie** is a cross-platform NES (Nintendo Entertainment System) emulator
written in Rust + TypeScript, packaged with [Tauri 2](https://v2.tauri.app/).
It runs on Windows, macOS, and Linux, lets you import your own iNES ROMs, organize
them into named collections, and play with full audio, video, and two-player
keyboard support.

> rs-nessie does not include or distribute any ROMs. You supply your own.

## Features

- iNES (`.nes`) ROM support, with mappers 0 (NROM), 1 (MMC1), 2 (UxROM),
  3 (CNROM), and 4 (MMC3).
- Full CPU + PPU + APU emulation written from scratch in pure Rust; runs at
  NES native refresh rate on commodity hardware.
- Library and collections: import your ROMs once, organize them into named
  groups, launch from a single click.
- Two-player local play with remappable per-player key bindings — two physical
  keyboards on one machine work simultaneously.
- Fullscreen mode (default `F11`).
- Cartridge battery saves persisted per-ROM so progress survives restarts.
- Native installers: NSIS `.exe`, `.dmg`, `.deb`, `.rpm`. Built locally and via
  GitHub Actions on tagged releases.

## Documentation

| Doc | What's inside |
|---|---|
| [User guide](./docs/user-guide.md) | Install, import ROMs, collections, key bindings, fullscreen, troubleshooting. |
| [Development guide](./docs/development.md) | Prerequisites, building from source, running tests and benches. |
| [Architecture](./docs/architecture.md) | Component diagram, threading model, IPC contract, framebuffer/audio pipelines. |
| [Design decisions](./docs/design-decisions.md) | Why Tauri 2, in-tree emulator core, WebGL2 rendering, SHA-1 ROM identity, etc. |
| [Build and release](./docs/build-release.md) | Local installer scripts, the tagged-release workflow, signing / notarization. |
| [Contributing](./docs/contributing.md) | Branch model, code review expectations, how to add a new mapper. |
| [Documentation index](./docs/README.md) | The full `./docs/` table of contents. |

## Quick start (developers)

```bash
git clone https://github.com/<your-fork>/rs-nessie.git
cd rs-nessie
npm --prefix app install
npm --prefix app run tauri dev
```

Full prerequisites (Tauri system packages per OS) are in
[`./docs/development.md`](./docs/development.md).

## License

See [LICENSE](./LICENSE).
