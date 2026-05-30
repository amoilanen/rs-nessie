//! Conditional branch semantics: target address, taken/not-taken cycle count,
//! and page-cross penalty.

use super::*;

#[test]
fn branch_not_taken_consumes_two_cycles() {
    // BNE $+2 with Z=1 → not taken.
    let (cpu, _, c) = run_one(&[0xD0, 0x10], |cpu, _| cpu.set_flag(flag::Z, true));
    assert_eq!(cpu.pc, 0x8002);
    assert_eq!(c, 2);
}

#[test]
fn branch_taken_no_page_cross_consumes_three_cycles() {
    // BEQ +$10 with Z=1 → taken to $8012.
    let (cpu, _, c) = run_one(&[0xF0, 0x10], |cpu, _| cpu.set_flag(flag::Z, true));
    assert_eq!(cpu.pc, 0x8012);
    assert_eq!(c, 3);
}

#[test]
fn branch_backward_taken() {
    // BPL -2 from $8000: opcode at $8000, operand at $8001, PC after fetch=$8002,
    // target = $8002 + (-2) = $8000.
    let (cpu, _, c) = run_one(&[0x10, (-2i8) as u8], |cpu, _| cpu.set_flag(flag::N, false));
    assert_eq!(cpu.pc, 0x8000);
    assert_eq!(c, 3); // same page → +1
}

#[test]
fn branch_taken_with_page_cross_consumes_four_cycles() {
    // Place BNE at $80F0; branching forward +$20 → PC=$80F2+$20=$8112,
    // crosses page ($81 vs $80).
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x80F0;
    bus.load(0x80F0, &[0xD0, 0x20]);
    cpu.set_flag(flag::Z, false);
    let c = cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x8112);
    assert_eq!(c, 4);
}

#[test]
fn all_branch_opcodes_check_their_flag() {
    // BCC (carry clear)
    let (cpu, _, _) = run_one(&[0x90, 0x04], |cpu, _| cpu.set_flag(flag::C, false));
    assert_eq!(cpu.pc, 0x8006);
    // BCS (carry set)
    let (cpu, _, _) = run_one(&[0xB0, 0x04], |cpu, _| cpu.set_flag(flag::C, true));
    assert_eq!(cpu.pc, 0x8006);
    // BMI (negative)
    let (cpu, _, _) = run_one(&[0x30, 0x04], |cpu, _| cpu.set_flag(flag::N, true));
    assert_eq!(cpu.pc, 0x8006);
    // BPL (positive)
    let (cpu, _, _) = run_one(&[0x10, 0x04], |cpu, _| cpu.set_flag(flag::N, false));
    assert_eq!(cpu.pc, 0x8006);
    // BVC (overflow clear)
    let (cpu, _, _) = run_one(&[0x50, 0x04], |cpu, _| cpu.set_flag(flag::V, false));
    assert_eq!(cpu.pc, 0x8006);
    // BVS (overflow set)
    let (cpu, _, _) = run_one(&[0x70, 0x04], |cpu, _| cpu.set_flag(flag::V, true));
    assert_eq!(cpu.pc, 0x8006);
}
