//! Unit tests for the 6502 CPU core.
//!
//! Organized as small focused submodules per concern:
//!
//! - `opcodes` — table-driven smoke tests covering every addressing-mode
//!   variant of every documented opcode (flags, result, cycle count).
//! - `page_cross` — verifies that the page-cross-sensitive opcodes spend an
//!   extra cycle when the indexed address crosses a 256-byte page.
//! - `branches` — verifies that branches consume 0 cycles when not taken,
//!   1 cycle when taken without page-cross, and 2 cycles when the branch
//!   target lies on a new page.
//! - `stack` — push/pop semantics + SP wraparound.
//! - `interrupts` — RESET, NMI, IRQ, BRK vector and stack-frame behaviour.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use super::*;

/// Run a single instruction from a freshly-built CPU + memory image and
/// return `(cpu, bus, cycles_consumed)`.
fn run_one(prog: &[u8], setup: impl FnOnce(&mut Cpu, &mut SimpleBus)) -> (Cpu, SimpleBus, u32) {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    bus.load(0x8000, prog);
    setup(&mut cpu, &mut bus);
    let cycles = cpu.step(&mut bus);
    (cpu, bus, cycles)
}

mod branches;
mod interrupts;
mod opcodes;
mod page_cross;
mod stack;
