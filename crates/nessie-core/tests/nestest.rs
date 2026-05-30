//! Integration test: run the `nestest.nes` CPU validation ROM and compare the
//! produced trace line-for-line against a committed golden log.
//!
//! ## Golden log derivation
//!
//! `tests/fixtures/nestest.golden.log` is the canonical Nintendulator trace
//! (`https://www.qmtpro.com/~nes/misc/nestest.log`) with two transformations
//! applied offline:
//!
//! 1. The disassembly column (cols 16..=47) is dropped.
//! 2. The `PPU:DDD,DDD` field is dropped (the PPU is not implemented yet).
//!
//! Only the **documented-opcode** portion is retained: lines 1..=5003 of the
//! original log. Line 5004 begins the undocumented-opcode tests, which
//! `nessie-core` deliberately does not implement (FR-2).
//!
//! The exact awk transformation used to produce the golden log:
//!
//! ```text
//! awk 'NR<=5003 {
//!   pc = substr($0, 1, 4);
//!   b1 = substr($0, 7, 2);
//!   b2 = substr($0, 10, 2);
//!   b3 = substr($0, 13, 2);
//!   i = index($0, "A:");
//!   rest = substr($0, i);
//!   sub(/ PPU:[ ]*[0-9]+,[ ]*[0-9]+/, "", rest);
//!   bytes = b1;
//!   if (b2 != "  ") bytes = bytes " " b2; else bytes = bytes "   ";
//!   if (b3 != "  ") bytes = bytes " " b3; else bytes = bytes "   ";
//!   printf "%s  %s  %s\n", pc, bytes, rest;
//! }' nestest.log > nestest.golden.log
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nessie_core::apu::Apu;
use nessie_core::bus::{NesBus, CPU_RAM_SIZE};
use nessie_core::cart::parse_ines;
use nessie_core::controller::Controller;
use nessie_core::cpu::{Cpu, OPCODES};
use nessie_core::ppu::Ppu;

const NESTEST_ROM: &[u8] = include_bytes!("fixtures/nestest.nes");
const NESTEST_GOLDEN: &str = include_str!("fixtures/nestest.golden.log");

/// Mode used to compute the length (in bytes) of each instruction so the
/// tracer can print the full opcode byte sequence before executing it.
fn instr_len(opcode: u8) -> usize {
    use nessie_core::cpu::Mode;
    match OPCODES[opcode as usize].mode {
        Mode::Implicit | Mode::Accumulator => 1,
        Mode::Immediate
        | Mode::ZeroPage
        | Mode::ZeroPageX
        | Mode::ZeroPageY
        | Mode::IndexedIndirect
        | Mode::IndirectIndexed
        | Mode::Relative => 2,
        Mode::Absolute | Mode::AbsoluteX | Mode::AbsoluteY | Mode::Indirect => 3,
    }
}

/// Format one trace line in the format consumed by `nestest.golden.log`:
///
/// `PPPP  BB BB BB  A:AA X:XX Y:YY P:PP SP:SS CYC:N`
fn format_trace(cpu: &Cpu, bytes: &[u8]) -> String {
    let mut byte_field = String::with_capacity(8);
    for i in 0..3 {
        if i < bytes.len() {
            byte_field.push_str(&format!("{:02X}", bytes[i]));
        } else {
            byte_field.push_str("  ");
        }
        if i < 2 {
            byte_field.push(' ');
        }
    }
    format!(
        "{:04X}  {}  A:{:02X} X:{:02X} Y:{:02X} P:{:02X} SP:{:02X} CYC:{}",
        cpu.pc, byte_field, cpu.a, cpu.x, cpu.y, cpu.p, cpu.sp, cpu.cycles
    )
}

#[test]
fn nestest_documented_opcodes_match_golden_trace() {
    let mut cart = parse_ines(NESTEST_ROM).expect("nestest.nes must parse");
    let mut ram = [0u8; CPU_RAM_SIZE];
    let mut ppu = Ppu::new();
    let mut apu = Apu::new();
    let mut controllers = [Controller::new(); 2];
    let mut nmi_pending = false;
    let mut irq_pending = false;
    let mut extra_cycles: u32 = 0;
    let mut bus = NesBus {
        ram: &mut ram,
        cart: &mut cart,
        ppu: &mut ppu,
        apu: &mut apu,
        controllers: &mut controllers,
        nmi_pending: &mut nmi_pending,
        irq_pending: &mut irq_pending,
        extra_cycles: &mut extra_cycles,
    };
    let mut cpu = Cpu::new();

    // nestest's automated mode is entered by jumping directly to $C000 with the
    // canonical post-reset state: A=X=Y=0, SP=$FD, P=$24, and 7 cycles already
    // spent emulating the reset sequence on real hardware.
    cpu.pc = 0xC000;
    cpu.sp = 0xFD;
    cpu.p = 0x24;
    cpu.cycles = 7;

    let golden: Vec<&str> = NESTEST_GOLDEN.lines().collect();
    let mut produced = Vec::with_capacity(golden.len());

    for line_no in 0..golden.len() {
        // Read the opcode bytes for the trace BEFORE executing (PC/A/X/etc. must
        // reflect the pre-step state).
        let opcode = nessie_core::cpu::CpuBus::read(&mut bus, cpu.pc);
        let len = instr_len(opcode);
        let mut bytes = Vec::with_capacity(len);
        for i in 0..len {
            bytes.push(nessie_core::cpu::CpuBus::read(
                &mut bus,
                cpu.pc.wrapping_add(i as u16),
            ));
        }

        let line = format_trace(&cpu, &bytes);
        produced.push(line.clone());

        if line != golden[line_no] {
            let context_start = line_no.saturating_sub(3);
            let mut diag = String::new();
            diag.push_str("nestest trace divergence:\n");
            for (i, prev) in golden.iter().enumerate().take(line_no).skip(context_start) {
                diag.push_str(&format!("  ok   {:5} | {}\n", i + 1, prev));
            }
            diag.push_str(&format!(
                "  WANT {:5} | {}\n  GOT  {:5} | {}\n",
                line_no + 1,
                golden[line_no],
                line_no + 1,
                line
            ));
            panic!("{}", diag);
        }

        cpu.step(&mut bus);
    }

    assert_eq!(produced.len(), golden.len());
}
