//! Page-cross-penalty regression tests.
//!
//! On the NMOS 6502 these opcodes spend one extra cycle when indexed
//! addressing crosses a 256-byte page boundary:
//!
//! - `LDA/LDX/LDY abs,X` and `abs,Y`
//! - `LDA (zp),Y`
//! - Read-only ALU ops with the same modes (`AND`, `EOR`, `ORA`, `ADC`,
//!   `SBC`, `CMP`)
//! - Branches: 1 cycle when taken, +1 more when the branch target page differs

use super::*;

#[test]
fn lda_abs_x_no_page_cross_is_four_cycles() {
    let (cpu, _, c) = run_one(&[0xBD, 0x00, 0x12], |cpu, bus| {
        cpu.x = 0x10;
        bus.mem[0x1210] = 0x55;
    });
    assert_eq!(cpu.a, 0x55);
    assert_eq!(c, 4);
}

#[test]
fn lda_abs_x_page_cross_is_five_cycles() {
    let (cpu, _, c) = run_one(&[0xBD, 0xFF, 0x12], |cpu, bus| {
        cpu.x = 0x01;
        bus.mem[0x1300] = 0x99;
    });
    assert_eq!(cpu.a, 0x99);
    assert_eq!(c, 5);
}

#[test]
fn lda_abs_y_page_cross_is_five_cycles() {
    let (cpu, _, c) = run_one(&[0xB9, 0xFF, 0x12], |cpu, bus| {
        cpu.y = 0x02;
        bus.mem[0x1301] = 0x77;
    });
    assert_eq!(cpu.a, 0x77);
    assert_eq!(c, 5);
}

#[test]
fn lda_indirect_y_page_cross_is_six_cycles() {
    let (cpu, _, c) = run_one(&[0xB1, 0x20], |cpu, bus| {
        cpu.y = 0x05;
        bus.mem[0x20] = 0xFE; // ptr low
        bus.mem[0x21] = 0x12; // ptr high → ptr=$12FE
        bus.mem[0x1303] = 0x33; // $12FE + $05 = $1303 (crosses page)
    });
    assert_eq!(cpu.a, 0x33);
    assert_eq!(c, 6);
}

#[test]
fn lda_indirect_y_no_page_cross_is_five_cycles() {
    let (cpu, _, c) = run_one(&[0xB1, 0x20], |cpu, bus| {
        cpu.y = 0x05;
        bus.mem[0x20] = 0x00;
        bus.mem[0x21] = 0x12;
        bus.mem[0x1205] = 0x44;
    });
    assert_eq!(cpu.a, 0x44);
    assert_eq!(c, 5);
}

#[test]
fn sta_abs_x_does_not_have_page_cross_penalty() {
    // Stores always pay the fixed extra cycle regardless of page-cross — the
    // base table already accounts for it (5 cycles). Verify both branches.
    let (_, bus, c_no_cross) = run_one(&[0x9D, 0x00, 0x12], |cpu, _| {
        cpu.a = 0xAA;
        cpu.x = 0x10;
    });
    assert_eq!(bus.mem[0x1210], 0xAA);
    assert_eq!(c_no_cross, 5);

    let (_, bus, c_cross) = run_one(&[0x9D, 0xFF, 0x12], |cpu, _| {
        cpu.a = 0xBB;
        cpu.x = 0x01;
    });
    assert_eq!(bus.mem[0x1300], 0xBB);
    assert_eq!(c_cross, 5);
}
