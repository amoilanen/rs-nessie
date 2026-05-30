# rs-nessie Documentation

**rs-nessie** is a cross-platform NES emulator written in Rust + TypeScript (Tauri 2).

This directory is the canonical documentation set for the project. Start here, then
follow the link that matches your role.

## Index

- [User guide](./user-guide.md) — install the app, import ROMs, organize collections,
  play, configure key bindings, troubleshoot.
- [Development guide](./development.md) — prerequisites, how to build and run
  rs-nessie locally on macOS / Linux / Windows, layout of the workspace, how to
  run tests and benches.
- [Architecture](./architecture.md) — high-level component diagram, threading model,
  IPC contract, frame and audio pipelines.
- [Design decisions](./design-decisions.md) — context / decision / consequences
  log for the non-obvious calls the project makes.
- [Build and release](./build-release.md) — how to tag a release, how the
  GitHub Actions release workflow produces NSIS / DMG / DEB / RPM artifacts,
  how to run the local installer scripts, how to wire signing/notarization.
- [Contributing](./contributing.md) — branch model, commit style, how to file
  issues, how to add a new mapper, code review expectations.

## Related top-level files

- [Repository root README](../README.md) — short project pitch and entry links.
- [LICENSE](../LICENSE) — project license.
- [`./.zenflow/tasks/`](../.zenflow/tasks/) — task specs and implementation plans
  used during development; useful background but not authoritative for users.
