//! Smoke tests covering at least one variant per documented opcode for each
//! addressing mode it supports. These are *not* exhaustive — the nestest
//! integration test (under `crates/nessie-core/tests/nestest.rs`) is the
//! comprehensive correctness oracle. The unit tests here lock down the
//! one-shot result-and-flag contract so a regression is caught quickly with
//! a clear failure message.

use super::*;

#[test]
fn lda_immediate_sets_zn_flags() {
    let (cpu, _, cycles) = run_one(&[0xA9, 0x00], |_, _| {});
    assert_eq!(cpu.a, 0);
    assert!(cpu.get_flag(flag::Z));
    assert!(!cpu.get_flag(flag::N));
    assert_eq!(cycles, 2);

    let (cpu, _, cycles) = run_one(&[0xA9, 0x80], |_, _| {});
    assert_eq!(cpu.a, 0x80);
    assert!(!cpu.get_flag(flag::Z));
    assert!(cpu.get_flag(flag::N));
    assert_eq!(cycles, 2);
}

#[test]
fn lda_zero_page() {
    let (cpu, _, cycles) = run_one(&[0xA5, 0x10], |_, bus| bus.mem[0x10] = 0x42);
    assert_eq!(cpu.a, 0x42);
    assert_eq!(cycles, 3);
}

#[test]
fn lda_zero_page_x() {
    let (cpu, _, cycles) = run_one(&[0xB5, 0x10], |cpu, bus| {
        cpu.x = 0x05;
        bus.mem[0x15] = 0x77;
    });
    assert_eq!(cpu.a, 0x77);
    assert_eq!(cycles, 4);
}

#[test]
fn lda_absolute() {
    let (cpu, _, cycles) = run_one(&[0xAD, 0x34, 0x12], |_, bus| bus.mem[0x1234] = 0xAA);
    assert_eq!(cpu.a, 0xAA);
    assert_eq!(cycles, 4);
}

#[test]
fn lda_absolute_x_no_page_cross() {
    let (cpu, _, cycles) = run_one(&[0xBD, 0x00, 0x12], |cpu, bus| {
        cpu.x = 0x10;
        bus.mem[0x1210] = 0x55;
    });
    assert_eq!(cpu.a, 0x55);
    assert_eq!(cycles, 4);
}

#[test]
fn lda_absolute_y_no_page_cross() {
    let (cpu, _, cycles) = run_one(&[0xB9, 0x00, 0x12], |cpu, bus| {
        cpu.y = 0x20;
        bus.mem[0x1220] = 0x66;
    });
    assert_eq!(cpu.a, 0x66);
    assert_eq!(cycles, 4);
}

#[test]
fn lda_indexed_indirect() {
    let (cpu, _, cycles) = run_one(&[0xA1, 0x20], |cpu, bus| {
        cpu.x = 0x04;
        // ($20 + X) = $24 → low byte at $24, high byte at $25
        bus.mem[0x24] = 0x34;
        bus.mem[0x25] = 0x12;
        bus.mem[0x1234] = 0x99;
    });
    assert_eq!(cpu.a, 0x99);
    assert_eq!(cycles, 6);
}

#[test]
fn lda_indirect_indexed_no_page_cross() {
    let (cpu, _, cycles) = run_one(&[0xB1, 0x20], |cpu, bus| {
        cpu.y = 0x10;
        bus.mem[0x20] = 0x00;
        bus.mem[0x21] = 0x12;
        bus.mem[0x1210] = 0x88;
    });
    assert_eq!(cpu.a, 0x88);
    assert_eq!(cycles, 5);
}

#[test]
fn ldx_and_ldy_immediate() {
    let (cpu, _, c1) = run_one(&[0xA2, 0xFF], |_, _| {});
    assert_eq!(cpu.x, 0xFF);
    assert!(cpu.get_flag(flag::N));
    assert_eq!(c1, 2);

    let (cpu, _, c2) = run_one(&[0xA0, 0x00], |_, _| {});
    assert_eq!(cpu.y, 0);
    assert!(cpu.get_flag(flag::Z));
    assert_eq!(c2, 2);
}

#[test]
fn sta_zero_page_writes_a() {
    let (cpu, bus, cycles) = run_one(&[0x85, 0x20], |cpu, _| cpu.a = 0x42);
    assert_eq!(cpu.a, 0x42);
    assert_eq!(bus.mem[0x20], 0x42);
    assert_eq!(cycles, 3);
}

#[test]
fn stx_and_sty_absolute() {
    let (cpu, bus, c1) = run_one(&[0x8E, 0x00, 0x03], |cpu, _| cpu.x = 0xAB);
    assert_eq!(bus.mem[0x0300], 0xAB);
    assert_eq!(c1, 4);
    assert_eq!(cpu.x, 0xAB);

    let (cpu, bus, c2) = run_one(&[0x8C, 0x00, 0x03], |cpu, _| cpu.y = 0xCD);
    assert_eq!(bus.mem[0x0300], 0xCD);
    assert_eq!(c2, 4);
    assert_eq!(cpu.y, 0xCD);
}

#[test]
fn transfer_instructions() {
    // TAX
    let (cpu, _, _) = run_one(&[0xAA], |cpu, _| cpu.a = 0x42);
    assert_eq!(cpu.x, 0x42);

    // TAY
    let (cpu, _, _) = run_one(&[0xA8], |cpu, _| cpu.a = 0x80);
    assert_eq!(cpu.y, 0x80);
    assert!(cpu.get_flag(flag::N));

    // TXA
    let (cpu, _, _) = run_one(&[0x8A], |cpu, _| cpu.x = 0x00);
    assert_eq!(cpu.a, 0);
    assert!(cpu.get_flag(flag::Z));

    // TYA
    let (cpu, _, _) = run_one(&[0x98], |cpu, _| cpu.y = 0x7F);
    assert_eq!(cpu.a, 0x7F);

    // TSX
    let (cpu, _, _) = run_one(&[0xBA], |cpu, _| cpu.sp = 0x10);
    assert_eq!(cpu.x, 0x10);

    // TXS (does NOT affect flags)
    let (cpu, _, _) = run_one(&[0x9A], |cpu, _| {
        cpu.x = 0x00;
        cpu.set_flag(flag::Z, false);
    });
    assert_eq!(cpu.sp, 0);
    assert!(!cpu.get_flag(flag::Z), "TXS must not touch flags");
}

#[test]
fn arithmetic_and_logic() {
    // ADC with carry-in: 0x10 + 0x20 + 1 = 0x31
    let (cpu, _, _) = run_one(&[0x69, 0x20], |cpu, _| {
        cpu.a = 0x10;
        cpu.set_flag(flag::C, true);
    });
    assert_eq!(cpu.a, 0x31);
    assert!(!cpu.get_flag(flag::C));
    assert!(!cpu.get_flag(flag::V));

    // ADC signed overflow: 0x50 + 0x50 = 0xA0 (V set, C clear)
    let (cpu, _, _) = run_one(&[0x69, 0x50], |cpu, _| cpu.a = 0x50);
    assert_eq!(cpu.a, 0xA0);
    assert!(cpu.get_flag(flag::V));

    // SBC: 0x50 - 0x10 (with carry set = no borrow) = 0x40
    let (cpu, _, _) = run_one(&[0xE9, 0x10], |cpu, _| {
        cpu.a = 0x50;
        cpu.set_flag(flag::C, true);
    });
    assert_eq!(cpu.a, 0x40);
    assert!(cpu.get_flag(flag::C), "no borrow → C stays set");

    // AND
    let (cpu, _, _) = run_one(&[0x29, 0x0F], |cpu, _| cpu.a = 0xF0);
    assert_eq!(cpu.a, 0x00);
    assert!(cpu.get_flag(flag::Z));

    // EOR
    let (cpu, _, _) = run_one(&[0x49, 0xFF], |cpu, _| cpu.a = 0x55);
    assert_eq!(cpu.a, 0xAA);

    // ORA
    let (cpu, _, _) = run_one(&[0x09, 0x0F], |cpu, _| cpu.a = 0xF0);
    assert_eq!(cpu.a, 0xFF);
    assert!(cpu.get_flag(flag::N));
}

#[test]
fn bit_zero_page_sets_v_and_n_from_memory() {
    let (cpu, _, cycles) = run_one(&[0x24, 0x10], |cpu, bus| {
        cpu.a = 0x0F;
        bus.mem[0x10] = 0xC0;
    });
    assert!(cpu.get_flag(flag::N));
    assert!(cpu.get_flag(flag::V));
    assert!(cpu.get_flag(flag::Z));
    assert_eq!(cycles, 3);
}

#[test]
fn compare_instructions() {
    // CMP equal → Z=1, C=1, N=0
    let (cpu, _, _) = run_one(&[0xC9, 0x10], |cpu, _| cpu.a = 0x10);
    assert!(cpu.get_flag(flag::Z));
    assert!(cpu.get_flag(flag::C));
    assert!(!cpu.get_flag(flag::N));

    // CMP A < operand → C=0, N from result
    let (cpu, _, _) = run_one(&[0xC9, 0x20], |cpu, _| cpu.a = 0x10);
    assert!(!cpu.get_flag(flag::C));
    assert!(cpu.get_flag(flag::N));

    // CPX immediate equal
    let (cpu, _, _) = run_one(&[0xE0, 0x05], |cpu, _| cpu.x = 0x05);
    assert!(cpu.get_flag(flag::Z));

    // CPY immediate
    let (cpu, _, _) = run_one(&[0xC0, 0x06], |cpu, _| cpu.y = 0x10);
    assert!(cpu.get_flag(flag::C));
}

#[test]
fn inc_dec_memory_and_registers() {
    let (cpu, bus, cycles) = run_one(&[0xE6, 0x10], |_, bus| bus.mem[0x10] = 0xFF);
    assert_eq!(bus.mem[0x10], 0x00);
    assert!(cpu.get_flag(flag::Z));
    assert_eq!(cycles, 5);

    let (cpu, bus, _) = run_one(&[0xC6, 0x10], |_, bus| bus.mem[0x10] = 0x01);
    assert_eq!(bus.mem[0x10], 0x00);
    assert!(cpu.get_flag(flag::Z));

    let (cpu, _, _) = run_one(&[0xE8], |cpu, _| cpu.x = 0x7F);
    assert_eq!(cpu.x, 0x80);
    assert!(cpu.get_flag(flag::N));

    let (cpu, _, _) = run_one(&[0xC8], |cpu, _| cpu.y = 0xFF);
    assert_eq!(cpu.y, 0x00);
    assert!(cpu.get_flag(flag::Z));

    let (cpu, _, _) = run_one(&[0xCA], |cpu, _| cpu.x = 0x01);
    assert_eq!(cpu.x, 0x00);
    assert!(cpu.get_flag(flag::Z));

    let (cpu, _, _) = run_one(&[0x88], |cpu, _| cpu.y = 0x80);
    assert_eq!(cpu.y, 0x7F);
}

#[test]
fn shift_and_rotate_accumulator() {
    // ASL A: 0x81 → 0x02, C=1
    let (cpu, _, c) = run_one(&[0x0A], |cpu, _| cpu.a = 0x81);
    assert_eq!(cpu.a, 0x02);
    assert!(cpu.get_flag(flag::C));
    assert_eq!(c, 2);

    // LSR A: 0x01 → 0x00, C=1, Z=1
    let (cpu, _, c) = run_one(&[0x4A], |cpu, _| cpu.a = 0x01);
    assert_eq!(cpu.a, 0);
    assert!(cpu.get_flag(flag::C));
    assert!(cpu.get_flag(flag::Z));
    assert_eq!(c, 2);

    // ROL A with C=1: 0x40 → 0x81, C=0
    let (cpu, _, _) = run_one(&[0x2A], |cpu, _| {
        cpu.a = 0x40;
        cpu.set_flag(flag::C, true);
    });
    assert_eq!(cpu.a, 0x81);
    assert!(!cpu.get_flag(flag::C));

    // ROR A with C=1: 0x02 → 0x81, C=0
    let (cpu, _, _) = run_one(&[0x6A], |cpu, _| {
        cpu.a = 0x02;
        cpu.set_flag(flag::C, true);
    });
    assert_eq!(cpu.a, 0x81);
    assert!(!cpu.get_flag(flag::C));
}

#[test]
fn shift_zero_page_writes_back_to_memory() {
    let (_, bus, cycles) = run_one(&[0x06, 0x20], |_, bus| bus.mem[0x20] = 0x01);
    assert_eq!(bus.mem[0x20], 0x02);
    assert_eq!(cycles, 5);
}

#[test]
fn jmp_absolute_and_indirect() {
    let (cpu, _, cycles) = run_one(&[0x4C, 0x34, 0x12], |_, _| {});
    assert_eq!(cpu.pc, 0x1234);
    assert_eq!(cycles, 3);

    // Indirect JMP with the classic page-wrap bug: vector at $02FF points
    // to ($02FF) and ($0200) (NOT $0300), so low/high are 0x34/0x12.
    let (cpu, _, cycles) = run_one(&[0x6C, 0xFF, 0x02], |_, bus| {
        bus.mem[0x02FF] = 0x34;
        bus.mem[0x0200] = 0x12;
        bus.mem[0x0300] = 0x99; // should NOT be used
    });
    assert_eq!(cpu.pc, 0x1234);
    assert_eq!(cycles, 5);
}

#[test]
fn flag_setters_and_clears() {
    // SEC, SEI, SED, CLC, CLI, CLD, CLV
    let (cpu, _, _) = run_one(&[0x38], |_, _| {}); // SEC
    assert!(cpu.get_flag(flag::C));
    let (cpu, _, _) = run_one(&[0x78], |_, _| {}); // SEI
    assert!(cpu.get_flag(flag::I));
    let (cpu, _, _) = run_one(&[0xF8], |_, _| {}); // SED
    assert!(cpu.get_flag(flag::D));
    let (cpu, _, _) = run_one(&[0x18], |cpu, _| cpu.set_flag(flag::C, true));
    assert!(!cpu.get_flag(flag::C));
    let (cpu, _, _) = run_one(&[0x58], |cpu, _| cpu.set_flag(flag::I, true));
    assert!(!cpu.get_flag(flag::I));
    let (cpu, _, _) = run_one(&[0xD8], |cpu, _| cpu.set_flag(flag::D, true));
    assert!(!cpu.get_flag(flag::D));
    let (cpu, _, _) = run_one(&[0xB8], |cpu, _| cpu.set_flag(flag::V, true));
    assert!(!cpu.get_flag(flag::V));
}

#[test]
fn nop_is_two_cycles() {
    let (_, _, cycles) = run_one(&[0xEA], |_, _| {});
    assert_eq!(cycles, 2);
}
