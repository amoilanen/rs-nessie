# `nessie-core` Test Fixtures

This directory contains read-only binary fixtures referenced by integration
tests under `./crates/nessie-core/tests/`.

## `nestest.nes`

The well-known **nestest** CPU validation ROM written by *Kevtris*. It is a
de-facto industry standard for verifying 6502 behaviour in NES emulators.

- Source mirror: <https://www.qmtpro.com/~nes/misc/nestest.nes>
- SHA-1: `5b608f023b41399c34dfc6c847d8af084e0f7aeb`
- Size: 24,592 bytes (16-byte iNES header + 16 KB PRG + 8 KB CHR)
- Licence: Freely redistributable homebrew test ROM. The author has placed it
  in the public domain for emulator development use.

The ROM contains a self-checking test harness. When entered at `$C000` (the
"automated mode" entry point used by this test), it walks through every
documented (and then undocumented) 6502 opcode and records the result in
zero-page locations `$02` / `$03`.

## `nestest.golden.log`

A **derivative** of the canonical Nintendulator trace
(<https://www.qmtpro.com/~nes/misc/nestest.log>) produced by stripping fields
that are out of scope for the CPU-only milestone:

1. The disassembly column (columns 16–47 inclusive) is **dropped**. The CPU
   tracer does not emit a disassembler; the opcode bytes already encode the
   instruction.
2. The `PPU:DDD,DDD` field is **dropped**. The PPU is implemented in a later
   step; only CPU registers and the cumulative CPU cycle count are compared.
3. CRLF line endings are normalized to LF. All 8,991 lines are kept: the
   documented-opcode tests (lines 1–5,003) and the unofficial-opcode tests
   that start at line 5,004 (`*NOP $A9`).

The trimmed format produced and consumed by `tests/nestest.rs` is:

```
PPPP  BB BB BB  A:AA X:XX Y:YY P:PP SP:SS CYC:N
```

The transformation script used to produce this file from the upstream log is
recorded in the integration test's module-level comment for reproducibility.

## `smoke.nes`

A minimal homebrew NROM ROM authored from scratch for this repository as an
end-to-end smoke test. It is **CC0** / public domain and is intentionally
trivial so the smoke test's expected framebuffer hash is stable across all
target platforms.

- Size: 24,592 bytes (16-byte iNES header + 16 KB PRG + 8 KB CHR)
- Mapper: 0 (NROM), horizontal mirroring, no battery
- Behavior:
  1. Disable interrupts, set the stack pointer.
  2. Enable APU pulse channel 1 via `$4015`.
  3. Configure pulse 1 with duty `10`, constant volume `0xF`, timer `0x200`,
     length counter index `0` so it emits a continuous tone.
  4. Jump-loop forever. Rendering is **never** enabled, so the PPU's
     framebuffer shows the power-on backdrop colour (NES colour `$00`) for the
     duration of the test.
- Reset vector: `$C000`. NMI / IRQ vectors point at the infinite loop.

The exact build recipe is committed at `scripts/build-smoke-rom.py` (kept in
sync with this file). The recipe deterministically reproduces the byte
sequence committed here.

## `nes15-NTSC.nes`

*nes15* by Mathew Brenaman — an implementation of the Fifteen Puzzle with
background music (NROM-128: 16 KB PRG, 8 KB CHR). Used by
`tests/game_nes15.rs` (and the host's command-layer test) to play a real
game end to end: boot → title screen → Start → move the cursor → slide a
tile.

- Source: <https://github.com/christopherpow/nes-test-roms> (`nes15-1.0.0/`,
  commit `95d8f621ae55cee0d09b91519a8989ae0e64753b`)
- SHA-1: `8fcc5798252370c63a98e7421131abf3eb22bfcf`
- Size: 28,688 bytes
- Licence: BSD-2-Clause, reproduced here as the licence requires:

```text
Copyright (c) 2011 Mathew Brenaman.  All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice,
   this list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE
ARE DISCLAIMED.  IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE
LIABLE FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR
CONSEQUENTIAL DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF
SUBSTITUTE GOODS OR SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS
INTERRUPTION) HOWEVER CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN
CONTRACT, STRICT LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE)
ARISING IN ANY WAY OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE
POSSIBILITY OF SUCH DAMAGE.
```

## blargg accuracy test ROMs (not vendored)

`tests/blargg.rs` runs blargg's public CPU/PPU/APU/MMC3 test ROMs. They are
fetched on demand by `scripts/fetch-test-roms.sh` (pinned commit of
<https://github.com/christopherpow/nes-test-roms>) into
`target/nes-test-roms`, because the collection carries no explicit licence.
