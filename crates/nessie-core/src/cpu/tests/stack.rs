//! Stack semantics: PHA/PLA/PHP/PLP, JSR/RTS, RTI, and the standard
//! `SP = $0100 | sp_low` page-1 layout.

use super::*;

#[test]
fn pha_decrements_sp_and_writes_to_stack_page() {
    let (cpu, bus, c) = run_one(&[0x48], |cpu, _| {
        cpu.sp = 0xFD;
        cpu.a = 0x42;
    });
    assert_eq!(cpu.sp, 0xFC);
    assert_eq!(bus.mem[0x01FD], 0x42);
    assert_eq!(c, 3);
}

#[test]
fn pla_increments_sp_and_reads_from_stack_page() {
    let (cpu, _, c) = run_one(&[0x68], |cpu, bus| {
        cpu.sp = 0xFC;
        bus.mem[0x01FD] = 0x99;
    });
    assert_eq!(cpu.sp, 0xFD);
    assert_eq!(cpu.a, 0x99);
    assert!(cpu.get_flag(flag::N));
    assert_eq!(c, 4);
}

#[test]
fn php_pushes_p_with_b_and_u_set() {
    let (_, bus, c) = run_one(&[0x08], |cpu, _| {
        cpu.sp = 0xFD;
        cpu.p = flag::I; // only I before PHP
    });
    assert_eq!(bus.mem[0x01FD], flag::I | flag::B | flag::U);
    assert_eq!(c, 3);
}

#[test]
fn plp_loads_p_clears_b_and_forces_u() {
    let (cpu, _, c) = run_one(&[0x28], |cpu, bus| {
        cpu.sp = 0xFC;
        bus.mem[0x01FD] = flag::C | flag::B; // B should be dropped, U forced on
    });
    assert_eq!(cpu.p & flag::B, 0);
    assert_ne!(cpu.p & flag::U, 0);
    assert_ne!(cpu.p & flag::C, 0);
    assert_eq!(c, 4);
}

#[test]
fn jsr_pushes_return_minus_one_and_rts_returns_to_next_byte() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    cpu.sp = 0xFD;
    // JSR $9000 ; LDA #$01 ; (subroutine at $9000: RTS)
    bus.load(0x8000, &[0x20, 0x00, 0x90, 0xA9, 0x01]);
    bus.mem[0x9000] = 0x60; // RTS

    let c_jsr = cpu.step(&mut bus);
    assert_eq!(cpu.pc, 0x9000);
    assert_eq!(c_jsr, 6);
    // Return address = $8000 + 3 - 1 = $8002
    assert_eq!(bus.mem[0x01FD], 0x80);
    assert_eq!(bus.mem[0x01FC], 0x02);
    assert_eq!(cpu.sp, 0xFB);

    let c_rts = cpu.step(&mut bus);
    assert_eq!(c_rts, 6);
    assert_eq!(cpu.pc, 0x8003); // popped $8002 → +1
    assert_eq!(cpu.sp, 0xFD);
}

#[test]
fn sp_wraps_around_from_00_to_ff_on_push() {
    let (cpu, bus, _) = run_one(&[0x48], |cpu, _| {
        cpu.sp = 0x00;
        cpu.a = 0xAA;
    });
    // Push wrote at $0100, then SP wrapped to $FF.
    assert_eq!(bus.mem[0x0100], 0xAA);
    assert_eq!(cpu.sp, 0xFF);
}
