# Build and Release

This document covers two related workflows:

1. **Local installer builds** — produce a single platform installer on your
   development machine.
2. **Tagged releases** — push a `v*` tag and let GitHub Actions build and
   publish installers for all three operating systems.

For everyday development (run, test, lint), see
[development.md](./development.md).

## Outputs

Per the spec and FR-28, the project produces four installer formats:

| OS | Format | Bundler |
|---|---|---|
| Windows | NSIS `.exe` | `tauri-bundler` → NSIS |
| macOS | universal `.dmg` (Apple Silicon + Intel) | `tauri-bundler` → DMG |
| Linux | `.deb` (Debian/Ubuntu) | `tauri-bundler` → DEB |
| Linux | `.rpm` (Fedora/RHEL/openSUSE) | `tauri-bundler` → RPM |

The Tauri config that drives all of this lives in
[`./app/src-tauri/tauri.conf.json`](../app/src-tauri/tauri.conf.json) under
`bundle.targets`.

## Local installer builds

Two scripts wrap `npm --prefix app run tauri build` for convenience and fail
fast on missing toolchains. They pick the right `--bundles` argument for the
host OS.

### POSIX (macOS / Linux)

```bash
./scripts/build-installer.sh
```

[`./scripts/build-installer.sh`](../scripts/build-installer.sh) uses
`set -euo pipefail` and prints the artifact paths it produced. On a developer
laptop it will produce:

- macOS host → a `.dmg` under
  `./app/src-tauri/target/release/bundle/dmg/` (or the universal target if
  both Apple Silicon and Intel toolchains are installed).
- Linux host → both a `.deb` and a `.rpm` under
  `./app/src-tauri/target/release/bundle/{deb,rpm}/`.

You can run targeted builds manually:

```bash
npm --prefix app run tauri build -- --bundles dmg
npm --prefix app run tauri build -- --bundles deb,rpm
```

### Windows

```powershell
./scripts/build-installer.ps1
```

[`./scripts/build-installer.ps1`](../scripts/build-installer.ps1) uses
`$ErrorActionPreference = 'Stop'`. It produces an NSIS `.exe` under
`./app/src-tauri/target/release/bundle/nsis/`.

Equivalent manual invocation:

```powershell
npm --prefix app run tauri build -- --bundles nsis
```

## Tagged releases (CI)

The release workflow is
[`./.github/workflows/release.yml`](../.github/workflows/release.yml). It is
triggered by pushing a tag matching `v*`:

```bash
git tag v0.1.0
git push origin v0.1.0
```

The workflow then runs a build matrix:

- **ubuntu-22.04** → `--bundles deb,rpm`
- **macos-14** → `--target universal-apple-darwin --bundles dmg`
- **windows-2022** → `--bundles nsis`

On each job:

1. Checkout, install Node 20, install the Rust stable toolchain.
2. Install Linux system dependencies (webkit2gtk-4.1, libsoup-3.0, alsa,
   rpm, fakeroot, dpkg) on the Ubuntu runner.
3. Restore the Cargo and `app/src-tauri/target/` caches.
4. `npm --prefix app ci` to install frontend dependencies deterministically.
5. `npm --prefix app run tauri build -- --bundles …` for that runner.
6. Collect artifacts into a `dist-artifacts/` directory.
7. Upload as a workflow artifact (retention: 14 days).
8. Upload to the GitHub Release for the tag with `gh release upload --clobber`.
   The release is created if it does not exist (the matrix races are tolerated
   by the `gh release view` precheck).

### Cutting a release — checklist

1. Make sure `main` is green on CI ([`./.github/workflows/ci.yml`](../.github/workflows/ci.yml)).
2. Bump the version in:
   - [`./app/package.json`](../app/package.json)
   - [`./app/src-tauri/Cargo.toml`](../app/src-tauri/Cargo.toml)
   - [`./app/src-tauri/tauri.conf.json`](../app/src-tauri/tauri.conf.json) (`version`)
   Keep them in sync.
3. Commit the version bump on `main`.
4. Tag and push:

   ```bash
   git tag v0.X.Y
   git push origin v0.X.Y
   ```

5. Watch the GitHub Actions [Release](../.github/workflows/release.yml) workflow.
   The four artifacts (`.dmg`, `.deb`, `.rpm`, `.exe`) appear on the GitHub
   Release page when all three matrix jobs are green.
6. Edit the Release body on GitHub to add release notes (the workflow seeds
   it with a placeholder).

## Signing and notarization

Per FR-31, the initial release does **not** require signing or notarization,
but the pipeline is wired so that adding them is just a matter of populating
the right secrets in the repository's Actions settings. The release workflow
reads these and silently builds unsigned bundles if any are absent.

### macOS (Apple notarization)

Configure these repository secrets:

| Secret | Purpose |
|---|---|
| `APPLE_ID` | Apple ID email used for notarization. |
| `APPLE_TEAM_ID` | Developer Team ID (10-character). |
| `APPLE_PASSWORD` | App-specific password (not the Apple ID password). |
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` Developer ID Application certificate. |
| `APPLE_CERTIFICATE_PASSWORD` | Password for the `.p12`. |
| `APPLE_SIGNING_IDENTITY` | The signing identity string (e.g. `Developer ID Application: Acme (TEAMID)`). |

These are referenced by `env:` blocks in
[`./.github/workflows/release.yml`](../.github/workflows/release.yml) lines
93–98. `tauri-bundler` picks them up and signs + notarizes the `.dmg`. If
absent, the workflow continues and produces an unsigned `.dmg`.

### Windows (Authenticode)

| Secret | Purpose |
|---|---|
| `WINDOWS_CERTIFICATE` | Base64-encoded `.pfx` code-signing certificate. |
| `WINDOWS_CERTIFICATE_PASSWORD` | Password for the `.pfx`. |

These are wired at lines 104–106 of the release workflow. If absent,
SmartScreen will warn users to "Run anyway" the first time.

### Linux (signed packages)

Optional, off by default:

| Secret | Purpose |
|---|---|
| `TAURI_SIGNING_PRIVATE_KEY` | Tauri updater signing key. |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for the above. |

These are present at lines 85–86. The Tauri updater is not enabled in the
initial release, so these are placeholders for future automatic updates.

## Bundle size guardrail (NFR-6)

The target is a Total installed size well under 200 MB excluding user ROM data.
The biggest contributors to bundle size are:

- The webview runtime (provided by the OS — does not increase the bundle).
- The Rust binary (release mode + LTO is enabled).
- The frontend JS/CSS bundle (Vite minifies for production).

If a release ever ships an unexpectedly large bundle, check:

1. `cargo bloat` on `rs-nessie` (workspace package).
2. The Vite bundle report (`npm --prefix app run build -- --analyze`).
3. That `tauri.conf.json` is not pulling in unused plugins or icons.

## Where to read more

- [Architecture](./architecture.md) — what is in the bundle and why.
- [Development guide](./development.md) — how to set up your toolchains.
- [Contributing](./contributing.md) — how to propose changes to the bundle or
  the release workflow.
