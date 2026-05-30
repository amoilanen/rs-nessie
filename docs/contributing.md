# Contributing

Thanks for your interest in rs-nessie! This guide explains how to file issues,
how to propose code changes, how the project is laid out, and the testing /
review bar we hold ourselves to.

If you are looking for the *what* of the project, start with the
[user guide](./user-guide.md). For the *how* of running it locally see
[development.md](./development.md).

## Code of Conduct

Be kind. Assume good intent. We follow the
[Contributor Covenant](https://www.contributor-covenant.org/version/2/1/code_of_conduct/);
violations can be reported privately to the project maintainers.

## Filing an issue

Before opening an issue, please:

1. Search existing issues — your problem may already be tracked.
2. Confirm you are running the latest release.
3. Reproduce on a clean configuration if possible (delete or move your
   `<config>/dev.rs-nessie/` to start fresh — see the
   [user guide](./user-guide.md#where-rs-nessie-stores-its-data)).

A good bug report includes:

- Your OS and version (e.g. "Windows 11 23H2", "macOS 14.4 Sonoma",
  "Ubuntu 22.04").
- The rs-nessie version (visible in the About dialog, or via the installer
  filename).
- Exact steps to reproduce.
- Expected vs. observed behavior.
- The contents of `rs-nessie.log` from your config directory, if relevant.
- For ROM-specific issues: the mapper number and a hash of the ROM. **Do not
  attach copyrighted ROMs.**

Feature requests are also welcome. Please describe the problem you are
trying to solve, not just the feature you imagine.

## Branch model and commit style

- `main` is the integration branch. It is protected — direct pushes are
  disabled; everything goes through a pull request.
- Feature work happens on short-lived topic branches (`feat/<short-name>`,
  `fix/<short-name>`).
- Commits should be focused and tell a story. Squash-merge is OK for small
  PRs; merge-commit is OK for multi-step PRs whose intermediate steps are
  themselves reviewable.
- Commit messages: imperative mood subject ("Add MMC5 mapper", not "Added
  MMC5"); body explains the *why*, not the *what*; reference issue numbers as
  `Fixes #N` / `Refs #N` when relevant.

## Pull requests

Open a pull request against `main`. Each PR should:

1. **Pass CI.** [`./.github/workflows/ci.yml`](../.github/workflows/ci.yml)
   runs lint + typecheck + unit/integration tests on Ubuntu, macOS, and
   Windows. A red CI is a blocker.
2. **Be focused.** One coherent change per PR. Refactors and feature work go
   in separate PRs whenever practical.
3. **Include tests.** Bug fixes get a regression test; new code gets unit
   tests next to the implementation. The standards are:
   - Rust: `#[cfg(test)] mod tests` for unit tests; `crates/*/tests/` for
     integration tests; criterion benches under `crates/*/benches/`.
   - TypeScript: colocated `*.test.ts(x)` files run by Vitest. UI tests use
     `@testing-library/react`.
4. **Update docs.** If the change is user-visible, update
   [user-guide.md](./user-guide.md). If it changes the architecture, update
   [architecture.md](./architecture.md). New design choices belong in
   [design-decisions.md](./design-decisions.md).
5. **Keep the bundle small.** Avoid pulling in heavyweight dependencies for
   small features. Run `cargo bloat` or the Vite bundle analyzer if you are
   not sure.

### PR checklist (copy into the PR description)

```
- [ ] Tests added / updated and passing locally
- [ ] `cargo fmt --all -- --check` passes
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes
- [ ] `npm --prefix app run typecheck` / `lint` / `test` pass
- [ ] User-facing or architectural docs updated as needed
- [ ] No copyrighted ROMs or proprietary assets included
```

## Code review expectations

- We review for **correctness, clarity, and maintainability** — in that order.
- Reviewers should leave concrete, actionable feedback. Authors should respond
  to every comment (resolve or push back); reviewers re-request review after
  changes.
- Two-eyes principle: at least one approval from a maintainer is required to
  merge. Trivial changes (typo fixes, documentation tweaks) can be self-merged
  with a single approval.
- Be mindful of cross-platform behavior. If a change touches anything
  OS-specific (file paths, audio device handling, key codes), the reviewer
  should call out the matrix surface.

## Coding standards

### Rust

- Idiomatic Rust 2021. Prefer iterators, `?` for error propagation, and
  zero-cost abstractions over runtime polymorphism unless there is a clear
  need for `dyn Trait`.
- `unsafe_code = "deny"` is opted in per crate where appropriate; if you
  need `unsafe`, justify it in a comment and unit-test the boundary.
- `clippy::unwrap_used` is warned in non-test code. Use `expect("…")` with a
  message that explains *why* the unwrap is safe.
- `rustfmt` is the source of truth for formatting — run `cargo fmt`.
- Public modules and non-trivial logic must be documented with `///`
  doc-comments. Run `cargo doc --workspace --no-deps` locally to preview.

### TypeScript

- `strict: true` in [`./app/tsconfig.json`](../app/tsconfig.json). No
  `any` without justification; prefer `unknown` and narrow.
- React function components only; no class components.
- State management uses Zustand for shared state. Local component state stays
  in `useState`.
- Prettier and ESLint are the source of truth for formatting and style.

### Tests

- Unit tests sit next to the code they cover.
- Integration tests go under `tests/` (Rust) or as part of a route's
  `*.test.tsx` next to the component.
- Snapshot tests are used sparingly — the `AppError` JSON shape is one
  intentional case.

## How to add a new mapper

Mappers live in [`./crates/nessie-core/src/cart/`](../crates/nessie-core/src/cart/).
Each is its own file (`mapper_NNN.rs`) and implements the `Mapper` trait
defined in [`./crates/nessie-core/src/cart/mod.rs`](../crates/nessie-core/src/cart/mod.rs).

Steps:

1. Pick the mapper number from the
   [NESdev wiki](https://wiki.nesdev.org/w/index.php/Mapper). Read its spec
   carefully, especially CHR/PRG bank layouts and any side-channel behavior
   (IRQ counters, mirroring overrides).
2. Add a new file `./crates/nessie-core/src/cart/mapper_NNN.rs` with a
   `Mapper<NNN>` struct and an `impl Mapper for Mapper<NNN>`. Implement at
   minimum `cpu_read`, `cpu_write`, `ppu_read`, `ppu_write`. Override
   `step`, `irq_pending`, `battery_ram`, `load_battery` only when the mapper
   needs them.
3. Wire the dispatch in `./crates/nessie-core/src/cart/mod.rs`'s
   `parse_ines` — match the mapper number, instantiate the struct, return it
   as `Box<dyn Mapper>` (or whatever the existing dispatch returns).
4. Add unit tests in a `#[cfg(test)] mod tests` block at the bottom of the
   new file. Test:
   - Bank-switching mappings on writes to the relevant register ranges.
   - Mirroring control if the mapper modifies it.
   - IRQ counter behavior (if any) — IRQ should be raised at the documented
     scanline / cycle.
5. If you have a public-domain homebrew ROM that exercises the mapper, drop
   it under `./crates/nessie-core/tests/fixtures/` with provenance in the
   fixtures `README`, and add an integration test that runs ~120 frames and
   asserts the framebuffer hash matches a committed expected value.
6. Update [user-guide.md](./user-guide.md) and the FR-2 acceptance table in
   [`./.zenflow/tasks/create-a-nes-8bit-emulator-which-2e40/requirements.md`](../.zenflow/tasks/create-a-nes-8bit-emulator-which-2e40/requirements.md)
   if the new mapper expands the supported set.

The `Mapper` trait is intentionally narrow so new mappers don't need to
touch the bus, CPU, or PPU. If you find yourself wanting to widen it, that's
a sign the new mapper has cross-cutting concerns worth discussing in a PR or
issue first.

## Security / responsible disclosure

If you find a security issue (e.g. a crash on user-supplied ROM that can be
weaponized, an IPC command that escapes its allowlist), please **do not** open
a public issue. Email the maintainers instead. We aim to respond within 7
days.

## Thank you

We appreciate every issue, PR, and review. rs-nessie is a small project; high
quality contributions go a long way.
