//! RESET / NMI / IRQ / BRK vector and stack-frame semantics.

use super::*;

#[test]
fn reset_loads_pc_from_fffc_fffd_and_costs_seven_cycles() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    bus.mem[0xFFFC] = 0x34;
    bus.mem[0xFFFD] = 0x12;
    cpu.reset(&mut bus);
    assert_eq!(cpu.pc, 0x1234);
    assert_eq!(cpu.sp, 0xFD);
    assert!(cpu.get_flag(flag::I));
    assert_eq!(cpu.cycles, 7);
}

#[test]
fn nmi_pushes_pc_and_p_then_jumps_to_fffa_vector() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x1234;
    cpu.sp = 0xFD;
    cpu.p = flag::U; // U set, I clear
    bus.mem[0xFFFA] = 0x00;
    bus.mem[0xFFFB] = 0x40;
    bus.nmi = true;
    bus.load(0x1234, &[0xEA]); // doesn't matter — NMI fires before fetch

    let c = cpu.step(&mut bus);
    assert_eq!(c, 7);
    assert_eq!(cpu.pc, 0x4000);
    assert!(cpu.get_flag(flag::I));
    // Stack frame (in push order): PCH, PCL, P with B=0, U=1.
    assert_eq!(bus.mem[0x01FD], 0x12);
    assert_eq!(bus.mem[0x01FC], 0x34);
    assert_eq!(bus.mem[0x01FB], flag::U); // B clear, U set
    assert_eq!(cpu.sp, 0xFA);
}

#[test]
fn irq_is_masked_when_i_is_set() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    cpu.sp = 0xFD;
    cpu.set_flag(flag::I, true);
    bus.load(0x8000, &[0xEA]); // NOP
    bus.mem[0xFFFE] = 0x00;
    bus.mem[0xFFFF] = 0x60;
    bus.irq = true;

    let c = cpu.step(&mut bus);
    // I=1 masks IRQ, so the NOP runs normally.
    assert_eq!(cpu.pc, 0x8001);
    assert_eq!(c, 2);
}

#[test]
fn irq_taken_when_i_is_clear() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    cpu.sp = 0xFD;
    cpu.set_flag(flag::I, false);
    bus.load(0x8000, &[0xEA]);
    bus.mem[0xFFFE] = 0x55;
    bus.mem[0xFFFF] = 0xAA;
    bus.irq = true;

    let c = cpu.step(&mut bus);
    assert_eq!(c, 7);
    assert_eq!(cpu.pc, 0xAA55);
    assert!(cpu.get_flag(flag::I));
    // Pushed P must have B=0, U=1.
    assert_eq!(bus.mem[0x01FB] & flag::B, 0);
    assert_ne!(bus.mem[0x01FB] & flag::U, 0);
}

#[test]
fn brk_pushes_pc_plus_two_with_b_set_and_jumps_to_fffe() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    cpu.sp = 0xFD;
    cpu.p = flag::U;
    bus.load(0x8000, &[0x00, 0xAB]); // BRK + padding byte
    bus.mem[0xFFFE] = 0x00;
    bus.mem[0xFFFF] = 0x50;

    let c = cpu.step(&mut bus);
    assert_eq!(c, 7);
    assert_eq!(cpu.pc, 0x5000);
    assert!(cpu.get_flag(flag::I));
    // Return PC = $8002 (BRK + padding byte).
    assert_eq!(bus.mem[0x01FD], 0x80);
    assert_eq!(bus.mem[0x01FC], 0x02);
    // B must be set in the pushed P; U also set.
    assert_eq!(bus.mem[0x01FB] & flag::B, flag::B);
    assert_eq!(bus.mem[0x01FB] & flag::U, flag::U);
}

#[test]
fn rti_pops_p_then_pc() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    cpu.sp = 0xFA;
    bus.mem[0x01FB] = flag::C | flag::B; // pushed P (B should be discarded on pull)
    bus.mem[0x01FC] = 0x34; // PCL
    bus.mem[0x01FD] = 0x12; // PCH
    bus.load(0x8000, &[0x40]); // RTI

    let c = cpu.step(&mut bus);
    assert_eq!(c, 6);
    assert_eq!(cpu.pc, 0x1234);
    assert_eq!(cpu.p & flag::B, 0);
    assert_ne!(cpu.p & flag::U, 0);
    assert_ne!(cpu.p & flag::C, 0);
    assert_eq!(cpu.sp, 0xFD);
}

#[test]
fn nmi_overrides_irq_when_both_pending() {
    let mut cpu = Cpu::new();
    let mut bus = SimpleBus::new();
    cpu.pc = 0x8000;
    cpu.sp = 0xFD;
    cpu.set_flag(flag::I, false);
    bus.mem[0xFFFA] = 0x00;
    bus.mem[0xFFFB] = 0x80;
    bus.mem[0xFFFE] = 0x00;
    bus.mem[0xFFFF] = 0x90;
    bus.nmi = true;
    bus.irq = true;

    let _ = cpu.step(&mut bus);
    // NMI vector wins.
    assert_eq!(cpu.pc, 0x8000);
}
