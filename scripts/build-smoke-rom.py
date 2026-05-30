#!/usr/bin/env python3
"""Deterministically build `crates/nessie-core/tests/fixtures/smoke.nes`.

Re-run this script if the byte layout of the smoke ROM needs to change (e.g.
to exercise additional APU registers in the smoke test). The smoke test's
expected framebuffer SHA-1 must be updated in `tests/smoke.rs` after any
change to this script.

The ROM is a CC0 public-domain authored work; provenance lives in
`crates/nessie-core/tests/fixtures/README.md`.
"""
from __future__ import annotations
import hashlib
import pathlib

# 16 KB PRG, all zero by default.
prg = bytearray(16 * 1024)
code = bytes([
    0x78,                         # SEI
    0xD8,                         # CLD
    0xA2, 0xFF,                   # LDX #$FF
    0x9A,                         # TXS
    0xA9, 0x01,                   # LDA #$01
    0x8D, 0x15, 0x40,             # STA $4015 (enable pulse 1)
    0xA9, 0xBF,                   # LDA #$BF (duty 10, halt, const vol, vol=F)
    0x8D, 0x00, 0x40,             # STA $4000
    0xA9, 0x08,                   # LDA #$08
    0x8D, 0x01, 0x40,             # STA $4001 (sweep off)
    0xA9, 0x00,                   # LDA #$00
    0x8D, 0x02, 0x40,             # STA $4002 (timer lo)
    0xA9, 0x02,                   # LDA #$02
    0x8D, 0x03, 0x40,             # STA $4003 (timer hi + length)
])
loop_addr = 0xC000 + len(code)
code += bytes([
    0x4C, loop_addr & 0xFF, (loop_addr >> 8) & 0xFF,
])
prg[0:len(code)] = code
# Reset/NMI/IRQ vectors. PRG is mirrored to $C000..$FFFF for a 16 KB cart,
# so $FFFA..$FFFF live at PRG offsets 0x3FFA..0x3FFF.
prg[0x3FFA] = loop_addr & 0xFF
prg[0x3FFB] = (loop_addr >> 8) & 0xFF
prg[0x3FFC] = 0x00
prg[0x3FFD] = 0xC0
prg[0x3FFE] = loop_addr & 0xFF
prg[0x3FFF] = (loop_addr >> 8) & 0xFF

header = bytearray(16)
header[0:4] = b"NES\x1a"
header[4] = 1
header[5] = 1
header[6] = 0
header[7] = 0

chr = bytearray(8 * 1024)
rom = bytes(header) + bytes(prg) + bytes(chr)

out = (
    pathlib.Path(__file__).resolve().parent.parent
    / "crates"
    / "nessie-core"
    / "tests"
    / "fixtures"
    / "smoke.nes"
)
out.write_bytes(rom)
print(f"wrote {len(rom)} bytes to {out}")
print(f"sha1:  {hashlib.sha1(rom).hexdigest()}")
