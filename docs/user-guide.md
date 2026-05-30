# User Guide

This guide explains how to install rs-nessie, import ROMs into the library,
organize them into collections, play, configure controls, and troubleshoot
common problems.

## Installation

Download the installer for your operating system from the project's GitHub
[Releases](https://github.com/) page and run it.

| OS | Installer | Notes |
|---|---|---|
| Windows 10 / 11 (x64) | `rs-nessie_<version>_x64-setup.exe` (NSIS) | Double-click to install. The installer is unsigned in the initial release; Windows SmartScreen may warn — choose *More info → Run anyway*. |
| macOS 12+ (Apple Silicon and Intel) | `rs-nessie_<version>_universal.dmg` | Mount the disk image and drag rs-nessie into `Applications/`. The first launch may need a right-click → *Open* to bypass Gatekeeper for unsigned bundles. |
| Linux (Debian / Ubuntu) | `rs-nessie_<version>_amd64.deb` | `sudo dpkg -i rs-nessie_<version>_amd64.deb` |
| Linux (Fedora / RHEL / openSUSE) | `rs-nessie-<version>-1.x86_64.rpm` | `sudo rpm -i rs-nessie-<version>-1.x86_64.rpm` |

You can also build from source — see [development.md](./development.md).

## First launch

On first launch you land on the **Library** view. It is empty and shows an
"Import your first ROM" call to action. The library is stored under your OS
user-data directory and persists across restarts (see
[Where rs-nessie stores its data](#where-rs-nessie-stores-its-data) below).

> rs-nessie does **not** ship with any ROMs. You must supply your own,
> legally obtained, iNES `.nes` files.

## Importing ROMs

1. Click **Import ROM** in the Library header.
2. Pick a `.nes` file from the native file-open dialog.
3. The ROM is parsed, deduplicated by content hash (SHA-1), and added to the
   library. The display title defaults to the file's stem (e.g. `super-game.nes`
   → `super-game`). You can rename it later.

Imports are tolerant of NES 2.0 headers. Unsupported mappers (anything outside
`{0, 1, 2, 3, 4}` in the initial release) are rejected with a toast that names
the offending mapper number.

If you import the same ROM twice, rs-nessie recognizes the content hash and
updates the stored path rather than creating a duplicate entry.

## Quick play (without importing)

You can launch a `.nes` file directly from disk without adding it to the
library. From the Library view, choose **Open ROM…** to pick a file; rs-nessie
loads it and switches to the Game view. The library is left unchanged.

## Collections

Collections are user-defined named groups of ROMs. A single ROM may appear in
zero, one, or many collections.

- **Create**: in the Collections view, click the **+** button in the sidebar and
  enter a name. Names must be unique within the library.
- **Add ROMs**: open a collection and click **Add ROM**. Pick one or more ROMs
  from the modal picker.
- **Remove**: select a ROM inside a collection and click the trash icon. This
  only removes it from that collection; the ROM stays in the library.
- **Rename / Delete**: right-click (or use the kebab menu) on a collection in
  the sidebar. Deleting a collection asks for confirmation; it does not delete
  the underlying ROM entries.

Removing a ROM from the **library** (not a collection) also strips it from every
collection automatically.

## Playing

Click **Play** on any ROM card to enter the **Game** view. The game starts
immediately, audio plays through the system default output device, and the
window can be resized; the picture preserves NES aspect ratio with integer
scaling where possible (FR-17).

While in-game, an overlay HUD (auto-hides) gives you:

- **Pause / Resume**
- **Mute / Unmute**
- **Volume slider**
- **Back to library** — stops the current session and returns to the library.

If a battery-backed save game is detected (e.g. *Zelda*-style cartridges),
rs-nessie saves it under `<config>/dev.rs-nessie/saves/<sha1>.srm` so progress
survives across sessions.

If the file behind a library ROM has been moved or deleted, rs-nessie shows a
"ROM file missing" error on launch but leaves the rest of the library intact —
you can re-import the file from its new location to restore the entry.

## Default key bindings

The eight standard NES buttons are bound for both players out of the box.
Bindings use `KeyboardEvent.code` values (layout-independent — `KeyW` is the
physical W key regardless of QWERTY/AZERTY).

| Button | Player 1 | Player 2 |
|---|---|---|
| Up | `KeyW` | `ArrowUp` |
| Down | `KeyS` | `ArrowDown` |
| Left | `KeyA` | `ArrowLeft` |
| Right | `KeyD` | `ArrowRight` |
| A | `KeyJ` | `Numpad0` |
| B | `KeyK` | `NumpadDecimal` |
| Start | `Enter` | `NumpadEnter` |
| Select | `ShiftRight` | `NumpadAdd` |

These are also visible in the Settings view, where you can remap any of them.

## Two-player play

Both controllers are emulated simultaneously by mapping different keys to each
player. Two physical keyboards plugged into the same machine work fine — the
operating system delivers their key events to the same DOM listener, and
rs-nessie distinguishes Player 1 from Player 2 purely by the **key**, not the
device. There is no per-device routing in the initial release.

## Settings

The **Settings** view has three sections:

- **Key bindings**: a table with one row per NES button and two columns (P1 / P2).
  Click a cell to enter capture mode; the next key you press becomes the new
  binding. Press `Esc` to cancel. Duplicate bindings inside the same player's
  map are rejected (an inline error appears).
- **Audio**: master volume slider (0–100%) and mute toggle.
- **General**: a "Reset to defaults" button per player and a global one for all
  settings.

Settings persist across restarts (see [Where rs-nessie stores its data](#where-rs-nessie-stores-its-data)).

## Fullscreen

- **Toggle**: press `F11` or use the HUD's *Fullscreen* button.
- **Exit**: press `F11` again, or `Esc`.

Audio and input continue working in fullscreen.

## Where rs-nessie stores its data

All persistent state lives under the OS user-config directory (resolved via
`dirs::config_dir()`):

| OS | Path |
|---|---|
| Windows | `%APPDATA%\dev.rs-nessie\` (e.g. `C:\Users\<you>\AppData\Roaming\dev.rs-nessie\`) |
| macOS | `~/Library/Application Support/dev.rs-nessie/` |
| Linux | `$XDG_CONFIG_HOME/dev.rs-nessie/` (typically `~/.config/dev.rs-nessie/`) |

Inside that directory:

- `library.json` — your ROM entries and collections.
- `settings.json` — key bindings, volume, last window state.
- `saves/<sha1>.srm` — one battery save per cartridge (named by ROM content hash,
  so saves follow the cartridge even if you move the ROM file).
- `rs-nessie.log` — application log file (rotated on each run).

All files are plain JSON / raw bytes; back them up by copying the folder.

## Troubleshooting

- **"Invalid ROM"** — the file isn't iNES-formatted or its header is corrupt.
  Try a different dump.
- **"Unsupported mapper N"** — rs-nessie supports mappers 0, 1, 2, 3, 4 in the
  initial release. Other mappers will fail with a toast naming the mapper.
- **"ROM file missing"** — the file at the stored path has been moved or
  deleted. Re-import the file from its new location.
- **No audio / stuttering** — check the system default audio device. If you
  changed devices while rs-nessie was running, restart the app. Lowering other
  CPU-intensive workloads can help on older laptops.
- **Game runs slowly** — confirm you are running a release build (the installer
  is always release-built). On Linux, ensure your distro's audio stack is alive
  (PipeWire / PulseAudio).
- **Frozen window** — check `rs-nessie.log`. Most non-fatal errors are recoverable
  and shown as toasts; if a panic happens, the log contains the trace.

If a bug persists, please file an issue and attach the log file — see
[contributing.md](./contributing.md).
