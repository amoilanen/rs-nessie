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
3. Only the **documented-opcode** portion is retained: the first 5,003 lines.
   Line 5,004 (`*NOP $A9`) marks the start of the undocumented-opcode tests,
   which `nessie-core` deliberately does **not** implement (FR-2 covers only
   documented opcodes).

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
     framebuffer remains all zeros for the duration of the test.
- Reset vector: `$C000`. NMI / IRQ vectors point at the infinite loop.

The exact build recipe is committed at `scripts/build-smoke-rom.py` (kept in
sync with this file). The recipe deterministically reproduces the byte
sequence committed here.
