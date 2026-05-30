//! NMOS 6502 CPU core used by the NES.
//!
//! Scope:
//!
//! - All **documented** instructions (FR-2) at cycle-accurate counts including
//!   page-cross penalties on the standard read-modify and indexed read paths.
//! - Standard interrupt sequencing for `RESET`, `NMI`, `IRQ`, and the `BRK`
//!   software interrupt, each consuming the correct number of bus cycles and
//!   leaving the stack in the canonical post-push layout.
//! - **No** undocumented opcodes; any encountered opcode that is not in the
//!   official table is treated as a no-op consuming 2 cycles (matching the
//!   simplest plausible behaviour without polluting the trace with arbitrary
//!   effects). The integration test in `tests/nestest.rs` runs only the
//!   documented-opcode portion of the nestest trace so this never triggers
//!   there.
//! - **No** binary-coded-decimal arithmetic: the NES's 2A03 silicon disables
//!   it, so the `D` flag is preserved for completeness but `ADC`/`SBC` always
//!   compute in binary.
//!
//! The CPU does not own its memory. Instead, all bus traffic goes through the
//! [`CpuBus`] trait. The real NES wiring lives in `crate::bus::NesBus`; tests
//! can supply a flat [`SimpleBus`] backed by 64 KB of RAM so opcodes can be
//! exercised without parsing a cartridge.

use core::fmt;

/// Public status-flag bit positions on the 6502 P register.
pub mod flag {
    /// Carry — set on add overflow / clear on subtract borrow.
    pub const C: u8 = 1 << 0;
    /// Zero — set when the last result was `0x00`.
    pub const Z: u8 = 1 << 1;
    /// Interrupt disable — when set, maskable IRQs are ignored.
    pub const I: u8 = 1 << 2;
    /// Decimal — present in the P register but ignored by the NES's 2A03.
    pub const D: u8 = 1 << 3;
    /// Break flag — only meaningful in the byte pushed by BRK/PHP.
    pub const B: u8 = 1 << 4;
    /// Unused — physical bit 5 of P, always reads as 1 when pushed.
    pub const U: u8 = 1 << 5;
    /// Overflow — set when a signed add/sub overflowed.
    pub const V: u8 = 1 << 6;
    /// Negative — copy of bit 7 of the last result.
    pub const N: u8 = 1 << 7;
}

/// Read/write interface presented to the CPU.
///
/// Implementors are responsible for honouring all NES memory-map quirks
/// (mirrored RAM, PPU register side effects on read, mapper routing). The CPU
/// makes one logical access per `read`/`write` call.
pub trait CpuBus {
    /// Read a byte from the given 16-bit address.
    fn read(&mut self, addr: u16) -> u8;
    /// Write a byte to the given 16-bit address.
    fn write(&mut self, addr: u16, value: u8);
    /// True while the PPU is asserting `/NMI` (edge-triggered — the bus is
    /// responsible for clearing the line once consumed).
    fn poll_nmi(&mut self) -> bool {
        false
    }
    /// True while any device is asserting `/IRQ` (level-triggered).
    fn poll_irq(&mut self) -> bool {
        false
    }
}

/// The 6502 CPU core.
///
/// The struct stores only the architectural register file plus a few
/// host-side counters used for diagnostics; all memory lives behind the bus.
#[derive(Debug, Clone)]
pub struct Cpu {
    /// Accumulator.
    pub a: u8,
    /// X index register.
    pub x: u8,
    /// Y index register.
    pub y: u8,
    /// Stack pointer (low byte; the stack page is `$0100..=$01FF`).
    pub sp: u8,
    /// Program counter.
    pub pc: u16,
    /// Processor status register.
    pub p: u8,
    /// Total CPU cycles executed since power-on (host-side counter, not a real
    /// register).
    pub cycles: u64,
}

impl Default for Cpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Cpu {
    /// Power-on state matching the convention used by nestest and most other
    /// validators: A/X/Y=0, SP=$FD, P=$24 (I and U set), PC=0.
    pub fn new() -> Self {
        Self {
            a: 0,
            x: 0,
            y: 0,
            sp: 0xFD,
            pc: 0,
            p: flag::I | flag::U,
            cycles: 0,
        }
    }

    /// Perform the 7-cycle reset sequence: pulls the reset vector from
    /// `$FFFC/$FFFD`, sets `I`, resets `SP` to `$FD`. Matches what the NES does
    /// on power-on.
    pub fn reset<B: CpuBus>(&mut self, bus: &mut B) {
        let lo = u16::from(bus.read(0xFFFC));
        let hi = u16::from(bus.read(0xFFFD));
        self.pc = (hi << 8) | lo;
        self.sp = 0xFD;
        self.p = flag::I | flag::U;
        self.cycles = self.cycles.wrapping_add(7);
    }

    /// Run exactly one instruction (or service a pending interrupt). Returns
    /// the number of CPU cycles consumed.
    ///
    /// NMI is checked before IRQ; both are checked before fetching the next
    /// opcode. This matches the canonical 6502 priority ordering.
    pub fn step<B: CpuBus>(&mut self, bus: &mut B) -> u32 {
        if bus.poll_nmi() {
            return self.service_interrupt(bus, 0xFFFA, /*brk=*/ false);
        }
        if bus.poll_irq() && !self.get_flag(flag::I) {
            return self.service_interrupt(bus, 0xFFFE, /*brk=*/ false);
        }
        self.execute(bus)
    }

    /// Internal: service `NMI` or `IRQ` (also reused by `BRK` with `brk=true`).
    fn service_interrupt<B: CpuBus>(&mut self, bus: &mut B, vector: u16, brk: bool) -> u32 {
        // Hardware interrupts push PC unchanged, BRK pushes PC+2 (the caller
        // for BRK adjusts PC before calling).
        let pc = self.pc;
        self.push_u16(bus, pc);
        let mut p = self.p | flag::U;
        if brk {
            p |= flag::B;
        } else {
            p &= !flag::B;
        }
        self.push(bus, p);
        self.set_flag(flag::I, true);
        let lo = u16::from(bus.read(vector));
        let hi = u16::from(bus.read(vector.wrapping_add(1)));
        self.pc = (hi << 8) | lo;
        self.cycles = self.cycles.wrapping_add(7);
        7
    }

    /// Fetch + decode + execute a single instruction.
    fn execute<B: CpuBus>(&mut self, bus: &mut B) -> u32 {
        let opcode = self.fetch(bus);
        let (cycles, extra_page_cross) = INSTR_TABLE[opcode as usize];
        let info = OPCODES[opcode as usize];
        let start_cycles = self.cycles;
        let mut extra = 0u32;

        match info.mnemonic {
            Mnemonic::LDA => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.a = v;
                self.set_zn(v);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::LDX => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.x = v;
                self.set_zn(v);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::LDY => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.y = v;
                self.set_zn(v);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::STA => {
                let addr = self.operand_addr(bus, info.mode).0;
                bus.write(addr, self.a);
            }
            Mnemonic::STX => {
                let addr = self.operand_addr(bus, info.mode).0;
                bus.write(addr, self.x);
            }
            Mnemonic::STY => {
                let addr = self.operand_addr(bus, info.mode).0;
                bus.write(addr, self.y);
            }
            Mnemonic::TAX => {
                self.x = self.a;
                self.set_zn(self.x);
            }
            Mnemonic::TAY => {
                self.y = self.a;
                self.set_zn(self.y);
            }
            Mnemonic::TXA => {
                self.a = self.x;
                self.set_zn(self.a);
            }
            Mnemonic::TYA => {
                self.a = self.y;
                self.set_zn(self.a);
            }
            Mnemonic::TSX => {
                self.x = self.sp;
                self.set_zn(self.x);
            }
            Mnemonic::TXS => {
                self.sp = self.x;
            }
            Mnemonic::PHA => self.push(bus, self.a),
            Mnemonic::PHP => self.push(bus, self.p | flag::B | flag::U),
            Mnemonic::PLA => {
                let v = self.pop(bus);
                self.a = v;
                self.set_zn(v);
            }
            Mnemonic::PLP => {
                let v = self.pop(bus);
                // Bit 4 (B) is discarded on PLP, bit 5 (U) is always 1.
                self.p = (v & !flag::B) | flag::U;
            }
            Mnemonic::AND => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.a &= v;
                self.set_zn(self.a);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::EOR => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.a ^= v;
                self.set_zn(self.a);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::ORA => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.a |= v;
                self.set_zn(self.a);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::BIT => {
                let addr = self.operand_addr(bus, info.mode).0;
                let v = bus.read(addr);
                self.set_flag(flag::Z, (self.a & v) == 0);
                self.set_flag(flag::N, v & 0x80 != 0);
                self.set_flag(flag::V, v & 0x40 != 0);
            }
            Mnemonic::ADC => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.adc(v);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::SBC => {
                let (v, c) = self.read_operand(bus, info.mode);
                // SBC = ADC of inverted operand.
                self.adc(v ^ 0xFF);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::CMP => {
                let (v, c) = self.read_operand(bus, info.mode);
                self.compare(self.a, v);
                extra += if extra_page_cross { c } else { 0 };
            }
            Mnemonic::CPX => {
                let (v, _) = self.read_operand(bus, info.mode);
                self.compare(self.x, v);
            }
            Mnemonic::CPY => {
                let (v, _) = self.read_operand(bus, info.mode);
                self.compare(self.y, v);
            }
            Mnemonic::INC => {
                let addr = self.operand_addr(bus, info.mode).0;
                let v = bus.read(addr).wrapping_add(1);
                bus.write(addr, v);
                self.set_zn(v);
            }
            Mnemonic::INX => {
                self.x = self.x.wrapping_add(1);
                self.set_zn(self.x);
            }
            Mnemonic::INY => {
                self.y = self.y.wrapping_add(1);
                self.set_zn(self.y);
            }
            Mnemonic::DEC => {
                let addr = self.operand_addr(bus, info.mode).0;
                let v = bus.read(addr).wrapping_sub(1);
                bus.write(addr, v);
                self.set_zn(v);
            }
            Mnemonic::DEX => {
                self.x = self.x.wrapping_sub(1);
                self.set_zn(self.x);
            }
            Mnemonic::DEY => {
                self.y = self.y.wrapping_sub(1);
                self.set_zn(self.y);
            }
            Mnemonic::ASL => {
                self.rmw(bus, info.mode, |this, v| {
                    this.set_flag(flag::C, v & 0x80 != 0);
                    let r = v << 1;
                    this.set_zn(r);
                    r
                });
            }
            Mnemonic::LSR => {
                self.rmw(bus, info.mode, |this, v| {
                    this.set_flag(flag::C, v & 0x01 != 0);
                    let r = v >> 1;
                    this.set_zn(r);
                    r
                });
            }
            Mnemonic::ROL => {
                self.rmw(bus, info.mode, |this, v| {
                    let carry_in = if this.get_flag(flag::C) { 1 } else { 0 };
                    this.set_flag(flag::C, v & 0x80 != 0);
                    let r = (v << 1) | carry_in;
                    this.set_zn(r);
                    r
                });
            }
            Mnemonic::ROR => {
                self.rmw(bus, info.mode, |this, v| {
                    let carry_in = if this.get_flag(flag::C) { 0x80 } else { 0 };
                    this.set_flag(flag::C, v & 0x01 != 0);
                    let r = (v >> 1) | carry_in;
                    this.set_zn(r);
                    r
                });
            }
            Mnemonic::JMP => {
                let addr = self.operand_addr(bus, info.mode).0;
                self.pc = addr;
            }
            Mnemonic::JSR => {
                // JSR pushes (PC - 1) where PC is the address of the byte
                // *after* the second operand byte.
                let addr_lo = u16::from(self.fetch(bus));
                let addr_hi = u16::from(self.fetch(bus));
                let target = (addr_hi << 8) | addr_lo;
                let return_pc = self.pc.wrapping_sub(1);
                self.push_u16(bus, return_pc);
                self.pc = target;
            }
            Mnemonic::RTS => {
                let lo = u16::from(self.pop(bus));
                let hi = u16::from(self.pop(bus));
                self.pc = ((hi << 8) | lo).wrapping_add(1);
            }
            Mnemonic::RTI => {
                let p = self.pop(bus);
                self.p = (p & !flag::B) | flag::U;
                let lo = u16::from(self.pop(bus));
                let hi = u16::from(self.pop(bus));
                self.pc = (hi << 8) | lo;
            }
            Mnemonic::BCC => extra += self.branch(bus, !self.get_flag(flag::C)),
            Mnemonic::BCS => extra += self.branch(bus, self.get_flag(flag::C)),
            Mnemonic::BEQ => extra += self.branch(bus, self.get_flag(flag::Z)),
            Mnemonic::BNE => extra += self.branch(bus, !self.get_flag(flag::Z)),
            Mnemonic::BMI => extra += self.branch(bus, self.get_flag(flag::N)),
            Mnemonic::BPL => extra += self.branch(bus, !self.get_flag(flag::N)),
            Mnemonic::BVC => extra += self.branch(bus, !self.get_flag(flag::V)),
            Mnemonic::BVS => extra += self.branch(bus, self.get_flag(flag::V)),
            Mnemonic::CLC => self.set_flag(flag::C, false),
            Mnemonic::SEC => self.set_flag(flag::C, true),
            Mnemonic::CLI => self.set_flag(flag::I, false),
            Mnemonic::SEI => self.set_flag(flag::I, true),
            Mnemonic::CLV => self.set_flag(flag::V, false),
            Mnemonic::CLD => self.set_flag(flag::D, false),
            Mnemonic::SED => self.set_flag(flag::D, true),
            Mnemonic::NOP => {}
            Mnemonic::BRK => {
                // BRK pushes PC + 1 (the byte after the opcode is a padding
                // byte). Our PC has already advanced past the opcode in fetch.
                self.pc = self.pc.wrapping_add(1);
                return self.service_interrupt(bus, 0xFFFE, /*brk=*/ true);
            }
            Mnemonic::Unknown => {
                // Treat unknown/undocumented opcodes as 2-cycle NOPs. They are
                // never reached by nestest's documented-opcode portion.
            }
        }

        let total = cycles as u32 + extra;
        self.cycles = start_cycles.wrapping_add(total as u64);
        total
    }

    /// Helper for read-modify-write instructions (ASL/LSR/ROL/ROR/INC/DEC).
    /// Accumulator mode reads/writes `a`; memory modes go through the bus.
    fn rmw<B, F>(&mut self, bus: &mut B, mode: Mode, op: F)
    where
        B: CpuBus,
        F: FnOnce(&mut Self, u8) -> u8,
    {
        if matches!(mode, Mode::Accumulator) {
            let v = self.a;
            self.a = op(self, v);
        } else {
            let addr = self.operand_addr(bus, mode).0;
            let v = bus.read(addr);
            let r = op(self, v);
            bus.write(addr, r);
        }
    }

    /// 6502 add-with-carry semantics (binary mode — NES never uses BCD).
    fn adc(&mut self, v: u8) {
        let a = self.a as u16;
        let m = v as u16;
        let c = if self.get_flag(flag::C) { 1 } else { 0 };
        let sum = a + m + c;
        let result = sum as u8;
        self.set_flag(flag::C, sum > 0xFF);
        // Overflow: signed addition of two same-signed operands producing a
        // differently-signed result.
        let overflow = (!(self.a ^ v) & (self.a ^ result) & 0x80) != 0;
        self.set_flag(flag::V, overflow);
        self.a = result;
        self.set_zn(result);
    }

    fn compare(&mut self, reg: u8, m: u8) {
        let r = reg.wrapping_sub(m);
        self.set_flag(flag::C, reg >= m);
        self.set_zn(r);
    }

    /// Conditional branch helper. Returns extra cycles consumed (1 for taken,
    /// +1 more when the target lies on a different memory page).
    fn branch<B: CpuBus>(&mut self, bus: &mut B, take: bool) -> u32 {
        let offset = self.fetch(bus) as i8 as i16;
        if take {
            let old_pc = self.pc;
            let new_pc = (old_pc as i32 + offset as i32) as u16;
            self.pc = new_pc;
            if page_crossed(old_pc, new_pc) {
                2
            } else {
                1
            }
        } else {
            0
        }
    }

    /// Push a byte to the stack at `$0100 + SP`, then decrement SP.
    fn push<B: CpuBus>(&mut self, bus: &mut B, value: u8) {
        bus.write(0x0100 | u16::from(self.sp), value);
        self.sp = self.sp.wrapping_sub(1);
    }

    /// Pre-increment SP and pop the byte at `$0100 + SP`.
    fn pop<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        self.sp = self.sp.wrapping_add(1);
        bus.read(0x0100 | u16::from(self.sp))
    }

    fn push_u16<B: CpuBus>(&mut self, bus: &mut B, value: u16) {
        self.push(bus, (value >> 8) as u8);
        self.push(bus, value as u8);
    }

    /// Fetch a byte at `PC` and increment.
    fn fetch<B: CpuBus>(&mut self, bus: &mut B) -> u8 {
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }

    fn fetch_u16<B: CpuBus>(&mut self, bus: &mut B) -> u16 {
        let lo = u16::from(self.fetch(bus));
        let hi = u16::from(self.fetch(bus));
        (hi << 8) | lo
    }

    /// Read the operand of the current instruction (for value-consuming ops).
    /// Returns `(value, page_cross_penalty_cycles)`; the page-cross cycle is
    /// only counted for opcodes the cycle table flags as page-cross-sensitive.
    fn read_operand<B: CpuBus>(&mut self, bus: &mut B, mode: Mode) -> (u8, u32) {
        match mode {
            Mode::Immediate => (self.fetch(bus), 0),
            _ => {
                let (addr, extra) = self.operand_addr(bus, mode);
                (bus.read(addr), extra)
            }
        }
    }

    /// Compute the effective address of the current instruction's operand.
    /// Returns `(addr, page_cross_penalty_cycles)`.
    fn operand_addr<B: CpuBus>(&mut self, bus: &mut B, mode: Mode) -> (u16, u32) {
        match mode {
            Mode::Immediate => {
                // Caller should use `read_operand` for immediate ops; falling
                // back here returns the PC of the immediate byte and advances.
                let addr = self.pc;
                self.pc = self.pc.wrapping_add(1);
                (addr, 0)
            }
            Mode::ZeroPage => (u16::from(self.fetch(bus)), 0),
            Mode::ZeroPageX => {
                let base = self.fetch(bus);
                (u16::from(base.wrapping_add(self.x)), 0)
            }
            Mode::ZeroPageY => {
                let base = self.fetch(bus);
                (u16::from(base.wrapping_add(self.y)), 0)
            }
            Mode::Absolute => (self.fetch_u16(bus), 0),
            Mode::AbsoluteX => {
                let base = self.fetch_u16(bus);
                let addr = base.wrapping_add(u16::from(self.x));
                let extra = if page_crossed(base, addr) { 1 } else { 0 };
                (addr, extra)
            }
            Mode::AbsoluteY => {
                let base = self.fetch_u16(bus);
                let addr = base.wrapping_add(u16::from(self.y));
                let extra = if page_crossed(base, addr) { 1 } else { 0 };
                (addr, extra)
            }
            Mode::Indirect => {
                // Used only by JMP. Emulates the classic page-wrap bug: when
                // the pointer's low byte is `$FF`, the high byte is read from
                // the same page (no carry into the high byte).
                let ptr = self.fetch_u16(bus);
                let lo = u16::from(bus.read(ptr));
                let hi_addr = (ptr & 0xFF00) | u16::from((ptr as u8).wrapping_add(1));
                let hi = u16::from(bus.read(hi_addr));
                ((hi << 8) | lo, 0)
            }
            Mode::IndexedIndirect => {
                // (zp,X) — the pointer is at zero page address `base+X`, with
                // wrap inside zero page; the address bytes also wrap inside
                // zero page.
                let base = self.fetch(bus);
                let ptr = base.wrapping_add(self.x);
                let lo = u16::from(bus.read(u16::from(ptr)));
                let hi = u16::from(bus.read(u16::from(ptr.wrapping_add(1))));
                ((hi << 8) | lo, 0)
            }
            Mode::IndirectIndexed => {
                // (zp),Y — pointer at zero page `base`, then add Y to the
                // resulting address. Zero-page bytes wrap. Page-cross penalty
                // applies on the add.
                let base = self.fetch(bus);
                let lo = u16::from(bus.read(u16::from(base)));
                let hi = u16::from(bus.read(u16::from(base.wrapping_add(1))));
                let ptr = (hi << 8) | lo;
                let addr = ptr.wrapping_add(u16::from(self.y));
                let extra = if page_crossed(ptr, addr) { 1 } else { 0 };
                (addr, extra)
            }
            Mode::Accumulator | Mode::Implicit | Mode::Relative => {
                // These modes don't produce a memory operand. Returning 0 is
                // safe because the executor never asks for one.
                (0, 0)
            }
        }
    }

    /// Set Zero and Negative based on `v`.
    fn set_zn(&mut self, v: u8) {
        self.set_flag(flag::Z, v == 0);
        self.set_flag(flag::N, v & 0x80 != 0);
    }

    /// True iff the flag bit is set in P.
    pub fn get_flag(&self, mask: u8) -> bool {
        self.p & mask != 0
    }

    /// Set or clear a flag bit in P.
    pub fn set_flag(&mut self, mask: u8, on: bool) {
        if on {
            self.p |= mask;
        } else {
            self.p &= !mask;
        }
    }
}

/// True when `a` and `b` lie on different 256-byte pages.
#[inline]
fn page_crossed(a: u16, b: u16) -> bool {
    (a & 0xFF00) != (b & 0xFF00)
}

/// Supported addressing modes. The 6502 has 13 documented modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Implicit,
    Accumulator,
    Immediate,
    ZeroPage,
    ZeroPageX,
    ZeroPageY,
    Absolute,
    AbsoluteX,
    AbsoluteY,
    Indirect,
    IndexedIndirect,
    IndirectIndexed,
    Relative,
}

/// 6502 instruction mnemonic enum used for opcode dispatch.
///
/// Variants use the canonical three-letter MOS Technology assembler mnemonics
/// (`LDA`, `STX`, `JSR`, ...) which are universally recognized by anyone
/// familiar with the architecture. The clippy `upper_case_acronyms` lint is
/// silenced here to preserve that 50-year-old convention.
#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mnemonic {
    ADC,
    AND,
    ASL,
    BCC,
    BCS,
    BEQ,
    BIT,
    BMI,
    BNE,
    BPL,
    BRK,
    BVC,
    BVS,
    CLC,
    CLD,
    CLI,
    CLV,
    CMP,
    CPX,
    CPY,
    DEC,
    DEX,
    DEY,
    EOR,
    INC,
    INX,
    INY,
    JMP,
    JSR,
    LDA,
    LDX,
    LDY,
    LSR,
    NOP,
    ORA,
    PHA,
    PHP,
    PLA,
    PLP,
    ROL,
    ROR,
    RTI,
    RTS,
    SBC,
    SEC,
    SED,
    SEI,
    STA,
    STX,
    STY,
    TAX,
    TAY,
    TSX,
    TXA,
    TXS,
    TYA,
    Unknown,
}

impl fmt::Display for Mnemonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use Mnemonic::*;
        let s = match self {
            ADC => "ADC",
            AND => "AND",
            ASL => "ASL",
            BCC => "BCC",
            BCS => "BCS",
            BEQ => "BEQ",
            BIT => "BIT",
            BMI => "BMI",
            BNE => "BNE",
            BPL => "BPL",
            BRK => "BRK",
            BVC => "BVC",
            BVS => "BVS",
            CLC => "CLC",
            CLD => "CLD",
            CLI => "CLI",
            CLV => "CLV",
            CMP => "CMP",
            CPX => "CPX",
            CPY => "CPY",
            DEC => "DEC",
            DEX => "DEX",
            DEY => "DEY",
            EOR => "EOR",
            INC => "INC",
            INX => "INX",
            INY => "INY",
            JMP => "JMP",
            JSR => "JSR",
            LDA => "LDA",
            LDX => "LDX",
            LDY => "LDY",
            LSR => "LSR",
            NOP => "NOP",
            ORA => "ORA",
            PHA => "PHA",
            PHP => "PHP",
            PLA => "PLA",
            PLP => "PLP",
            ROL => "ROL",
            ROR => "ROR",
            RTI => "RTI",
            RTS => "RTS",
            SBC => "SBC",
            SEC => "SEC",
            SED => "SED",
            SEI => "SEI",
            STA => "STA",
            STX => "STX",
            STY => "STY",
            TAX => "TAX",
            TAY => "TAY",
            TSX => "TSX",
            TXA => "TXA",
            TXS => "TXS",
            TYA => "TYA",
            Unknown => "???",
        };
        f.write_str(s)
    }
}

/// Static metadata for one opcode byte: mnemonic + addressing mode.
#[derive(Debug, Clone, Copy)]
pub struct OpcodeInfo {
    pub mnemonic: Mnemonic,
    pub mode: Mode,
}

const fn op(mnemonic: Mnemonic, mode: Mode) -> OpcodeInfo {
    OpcodeInfo { mnemonic, mode }
}

const UNK: OpcodeInfo = op(Mnemonic::Unknown, Mode::Implicit);

/// 256-entry opcode → `(mnemonic, mode)` table.
/// Only documented opcodes are filled in; everything else is `UNK`.
pub const OPCODES: [OpcodeInfo; 256] = build_opcodes();

const fn build_opcodes() -> [OpcodeInfo; 256] {
    let mut t = [UNK; 256];
    use Mnemonic::*;
    use Mode::*;

    // BRK + ORA family
    t[0x00] = op(BRK, Implicit);
    t[0x01] = op(ORA, IndexedIndirect);
    t[0x05] = op(ORA, ZeroPage);
    t[0x06] = op(ASL, ZeroPage);
    t[0x08] = op(PHP, Implicit);
    t[0x09] = op(ORA, Immediate);
    t[0x0A] = op(ASL, Accumulator);
    t[0x0D] = op(ORA, Absolute);
    t[0x0E] = op(ASL, Absolute);

    // BPL / ORA
    t[0x10] = op(BPL, Relative);
    t[0x11] = op(ORA, IndirectIndexed);
    t[0x15] = op(ORA, ZeroPageX);
    t[0x16] = op(ASL, ZeroPageX);
    t[0x18] = op(CLC, Implicit);
    t[0x19] = op(ORA, AbsoluteY);
    t[0x1D] = op(ORA, AbsoluteX);
    t[0x1E] = op(ASL, AbsoluteX);

    // JSR / AND / BIT
    t[0x20] = op(JSR, Absolute);
    t[0x21] = op(AND, IndexedIndirect);
    t[0x24] = op(BIT, ZeroPage);
    t[0x25] = op(AND, ZeroPage);
    t[0x26] = op(ROL, ZeroPage);
    t[0x28] = op(PLP, Implicit);
    t[0x29] = op(AND, Immediate);
    t[0x2A] = op(ROL, Accumulator);
    t[0x2C] = op(BIT, Absolute);
    t[0x2D] = op(AND, Absolute);
    t[0x2E] = op(ROL, Absolute);

    t[0x30] = op(BMI, Relative);
    t[0x31] = op(AND, IndirectIndexed);
    t[0x35] = op(AND, ZeroPageX);
    t[0x36] = op(ROL, ZeroPageX);
    t[0x38] = op(SEC, Implicit);
    t[0x39] = op(AND, AbsoluteY);
    t[0x3D] = op(AND, AbsoluteX);
    t[0x3E] = op(ROL, AbsoluteX);

    // RTI / EOR / LSR / JMP / PHA
    t[0x40] = op(RTI, Implicit);
    t[0x41] = op(EOR, IndexedIndirect);
    t[0x45] = op(EOR, ZeroPage);
    t[0x46] = op(LSR, ZeroPage);
    t[0x48] = op(PHA, Implicit);
    t[0x49] = op(EOR, Immediate);
    t[0x4A] = op(LSR, Accumulator);
    t[0x4C] = op(JMP, Absolute);
    t[0x4D] = op(EOR, Absolute);
    t[0x4E] = op(LSR, Absolute);

    t[0x50] = op(BVC, Relative);
    t[0x51] = op(EOR, IndirectIndexed);
    t[0x55] = op(EOR, ZeroPageX);
    t[0x56] = op(LSR, ZeroPageX);
    t[0x58] = op(CLI, Implicit);
    t[0x59] = op(EOR, AbsoluteY);
    t[0x5D] = op(EOR, AbsoluteX);
    t[0x5E] = op(LSR, AbsoluteX);

    // RTS / ADC / ROR / JMP indirect / PLA
    t[0x60] = op(RTS, Implicit);
    t[0x61] = op(ADC, IndexedIndirect);
    t[0x65] = op(ADC, ZeroPage);
    t[0x66] = op(ROR, ZeroPage);
    t[0x68] = op(PLA, Implicit);
    t[0x69] = op(ADC, Immediate);
    t[0x6A] = op(ROR, Accumulator);
    t[0x6C] = op(JMP, Indirect);
    t[0x6D] = op(ADC, Absolute);
    t[0x6E] = op(ROR, Absolute);

    t[0x70] = op(BVS, Relative);
    t[0x71] = op(ADC, IndirectIndexed);
    t[0x75] = op(ADC, ZeroPageX);
    t[0x76] = op(ROR, ZeroPageX);
    t[0x78] = op(SEI, Implicit);
    t[0x79] = op(ADC, AbsoluteY);
    t[0x7D] = op(ADC, AbsoluteX);
    t[0x7E] = op(ROR, AbsoluteX);

    // STA / STX / STY / DEY / TXA
    t[0x81] = op(STA, IndexedIndirect);
    t[0x84] = op(STY, ZeroPage);
    t[0x85] = op(STA, ZeroPage);
    t[0x86] = op(STX, ZeroPage);
    t[0x88] = op(DEY, Implicit);
    t[0x8A] = op(TXA, Implicit);
    t[0x8C] = op(STY, Absolute);
    t[0x8D] = op(STA, Absolute);
    t[0x8E] = op(STX, Absolute);

    t[0x90] = op(BCC, Relative);
    t[0x91] = op(STA, IndirectIndexed);
    t[0x94] = op(STY, ZeroPageX);
    t[0x95] = op(STA, ZeroPageX);
    t[0x96] = op(STX, ZeroPageY);
    t[0x98] = op(TYA, Implicit);
    t[0x99] = op(STA, AbsoluteY);
    t[0x9A] = op(TXS, Implicit);
    t[0x9D] = op(STA, AbsoluteX);

    // LDY / LDX / LDA / TAY / TAX
    t[0xA0] = op(LDY, Immediate);
    t[0xA1] = op(LDA, IndexedIndirect);
    t[0xA2] = op(LDX, Immediate);
    t[0xA4] = op(LDY, ZeroPage);
    t[0xA5] = op(LDA, ZeroPage);
    t[0xA6] = op(LDX, ZeroPage);
    t[0xA8] = op(TAY, Implicit);
    t[0xA9] = op(LDA, Immediate);
    t[0xAA] = op(TAX, Implicit);
    t[0xAC] = op(LDY, Absolute);
    t[0xAD] = op(LDA, Absolute);
    t[0xAE] = op(LDX, Absolute);

    t[0xB0] = op(BCS, Relative);
    t[0xB1] = op(LDA, IndirectIndexed);
    t[0xB4] = op(LDY, ZeroPageX);
    t[0xB5] = op(LDA, ZeroPageX);
    t[0xB6] = op(LDX, ZeroPageY);
    t[0xB8] = op(CLV, Implicit);
    t[0xB9] = op(LDA, AbsoluteY);
    t[0xBA] = op(TSX, Implicit);
    t[0xBC] = op(LDY, AbsoluteX);
    t[0xBD] = op(LDA, AbsoluteX);
    t[0xBE] = op(LDX, AbsoluteY);

    // CPY / CMP / DEC / INY / DEX
    t[0xC0] = op(CPY, Immediate);
    t[0xC1] = op(CMP, IndexedIndirect);
    t[0xC4] = op(CPY, ZeroPage);
    t[0xC5] = op(CMP, ZeroPage);
    t[0xC6] = op(DEC, ZeroPage);
    t[0xC8] = op(INY, Implicit);
    t[0xC9] = op(CMP, Immediate);
    t[0xCA] = op(DEX, Implicit);
    t[0xCC] = op(CPY, Absolute);
    t[0xCD] = op(CMP, Absolute);
    t[0xCE] = op(DEC, Absolute);

    t[0xD0] = op(BNE, Relative);
    t[0xD1] = op(CMP, IndirectIndexed);
    t[0xD5] = op(CMP, ZeroPageX);
    t[0xD6] = op(DEC, ZeroPageX);
    t[0xD8] = op(CLD, Implicit);
    t[0xD9] = op(CMP, AbsoluteY);
    t[0xDD] = op(CMP, AbsoluteX);
    t[0xDE] = op(DEC, AbsoluteX);

    // CPX / SBC / INC / INX / NOP
    t[0xE0] = op(CPX, Immediate);
    t[0xE1] = op(SBC, IndexedIndirect);
    t[0xE4] = op(CPX, ZeroPage);
    t[0xE5] = op(SBC, ZeroPage);
    t[0xE6] = op(INC, ZeroPage);
    t[0xE8] = op(INX, Implicit);
    t[0xE9] = op(SBC, Immediate);
    t[0xEA] = op(NOP, Implicit);
    t[0xEC] = op(CPX, Absolute);
    t[0xED] = op(SBC, Absolute);
    t[0xEE] = op(INC, Absolute);

    t[0xF0] = op(BEQ, Relative);
    t[0xF1] = op(SBC, IndirectIndexed);
    t[0xF5] = op(SBC, ZeroPageX);
    t[0xF6] = op(INC, ZeroPageX);
    t[0xF8] = op(SED, Implicit);
    t[0xF9] = op(SBC, AbsoluteY);
    t[0xFD] = op(SBC, AbsoluteX);
    t[0xFE] = op(INC, AbsoluteX);

    t
}

/// Base cycle table and "is page-cross sensitive" bit per opcode.
///
/// Branch instructions list 2 here; the executor adds 1 (taken) or 1+1 (taken
/// and page-crossed) at run time.
pub const INSTR_TABLE: [(u8, bool); 256] = build_cycle_table();

const fn build_cycle_table() -> [(u8, bool); 256] {
    let mut t = [(2u8, false); 256];

    // (opcode, base_cycles, page_cross_sensitive)
    let entries: &[(u8, u8, bool)] = &[
        // BRK & friends
        (0x00, 7, false),
        (0x01, 6, false),
        (0x05, 3, false),
        (0x06, 5, false),
        (0x08, 3, false),
        (0x09, 2, false),
        (0x0A, 2, false),
        (0x0D, 4, false),
        (0x0E, 6, false),
        (0x10, 2, false),
        (0x11, 5, true),
        (0x15, 4, false),
        (0x16, 6, false),
        (0x18, 2, false),
        (0x19, 4, true),
        (0x1D, 4, true),
        (0x1E, 7, false),
        (0x20, 6, false),
        (0x21, 6, false),
        (0x24, 3, false),
        (0x25, 3, false),
        (0x26, 5, false),
        (0x28, 4, false),
        (0x29, 2, false),
        (0x2A, 2, false),
        (0x2C, 4, false),
        (0x2D, 4, false),
        (0x2E, 6, false),
        (0x30, 2, false),
        (0x31, 5, true),
        (0x35, 4, false),
        (0x36, 6, false),
        (0x38, 2, false),
        (0x39, 4, true),
        (0x3D, 4, true),
        (0x3E, 7, false),
        (0x40, 6, false),
        (0x41, 6, false),
        (0x45, 3, false),
        (0x46, 5, false),
        (0x48, 3, false),
        (0x49, 2, false),
        (0x4A, 2, false),
        (0x4C, 3, false),
        (0x4D, 4, false),
        (0x4E, 6, false),
        (0x50, 2, false),
        (0x51, 5, true),
        (0x55, 4, false),
        (0x56, 6, false),
        (0x58, 2, false),
        (0x59, 4, true),
        (0x5D, 4, true),
        (0x5E, 7, false),
        (0x60, 6, false),
        (0x61, 6, false),
        (0x65, 3, false),
        (0x66, 5, false),
        (0x68, 4, false),
        (0x69, 2, false),
        (0x6A, 2, false),
        (0x6C, 5, false),
        (0x6D, 4, false),
        (0x6E, 6, false),
        (0x70, 2, false),
        (0x71, 5, true),
        (0x75, 4, false),
        (0x76, 6, false),
        (0x78, 2, false),
        (0x79, 4, true),
        (0x7D, 4, true),
        (0x7E, 7, false),
        (0x81, 6, false),
        (0x84, 3, false),
        (0x85, 3, false),
        (0x86, 3, false),
        (0x88, 2, false),
        (0x8A, 2, false),
        (0x8C, 4, false),
        (0x8D, 4, false),
        (0x8E, 4, false),
        (0x90, 2, false),
        (0x91, 6, false),
        (0x94, 4, false),
        (0x95, 4, false),
        (0x96, 4, false),
        (0x98, 2, false),
        (0x99, 5, false),
        (0x9A, 2, false),
        (0x9D, 5, false),
        (0xA0, 2, false),
        (0xA1, 6, false),
        (0xA2, 2, false),
        (0xA4, 3, false),
        (0xA5, 3, false),
        (0xA6, 3, false),
        (0xA8, 2, false),
        (0xA9, 2, false),
        (0xAA, 2, false),
        (0xAC, 4, false),
        (0xAD, 4, false),
        (0xAE, 4, false),
        (0xB0, 2, false),
        (0xB1, 5, true),
        (0xB4, 4, false),
        (0xB5, 4, false),
        (0xB6, 4, false),
        (0xB8, 2, false),
        (0xB9, 4, true),
        (0xBA, 2, false),
        (0xBC, 4, true),
        (0xBD, 4, true),
        (0xBE, 4, true),
        (0xC0, 2, false),
        (0xC1, 6, false),
        (0xC4, 3, false),
        (0xC5, 3, false),
        (0xC6, 5, false),
        (0xC8, 2, false),
        (0xC9, 2, false),
        (0xCA, 2, false),
        (0xCC, 4, false),
        (0xCD, 4, false),
        (0xCE, 6, false),
        (0xD0, 2, false),
        (0xD1, 5, true),
        (0xD5, 4, false),
        (0xD6, 6, false),
        (0xD8, 2, false),
        (0xD9, 4, true),
        (0xDD, 4, true),
        (0xDE, 7, false),
        (0xE0, 2, false),
        (0xE1, 6, false),
        (0xE4, 3, false),
        (0xE5, 3, false),
        (0xE6, 5, false),
        (0xE8, 2, false),
        (0xE9, 2, false),
        (0xEA, 2, false),
        (0xEC, 4, false),
        (0xED, 4, false),
        (0xEE, 6, false),
        (0xF0, 2, false),
        (0xF1, 5, true),
        (0xF5, 4, false),
        (0xF6, 6, false),
        (0xF8, 2, false),
        (0xF9, 4, true),
        (0xFD, 4, true),
        (0xFE, 7, false),
    ];

    let mut i = 0;
    while i < entries.len() {
        let (op, c, pc) = entries[i];
        t[op as usize] = (c, pc);
        i += 1;
    }
    t
}

/// Convenience flat 64 KB bus used by tests.
pub struct SimpleBus {
    /// Full 64 KB linear memory.
    pub mem: Box<[u8; 0x10000]>,
    /// Setting these flips `poll_nmi` / `poll_irq`.
    pub nmi: bool,
    pub irq: bool,
}

impl Default for SimpleBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SimpleBus {
    /// Allocate a zeroed 64 KB memory image.
    pub fn new() -> Self {
        Self {
            mem: Box::new([0u8; 0x10000]),
            nmi: false,
            irq: false,
        }
    }

    /// Copy `bytes` into memory starting at `addr`.
    pub fn load(&mut self, addr: u16, bytes: &[u8]) {
        let start = addr as usize;
        let end = start + bytes.len();
        self.mem[start..end].copy_from_slice(bytes);
    }
}

impl CpuBus for SimpleBus {
    fn read(&mut self, addr: u16) -> u8 {
        self.mem[addr as usize]
    }
    fn write(&mut self, addr: u16, value: u8) {
        self.mem[addr as usize] = value;
    }
    fn poll_nmi(&mut self) -> bool {
        let n = self.nmi;
        self.nmi = false;
        n
    }
    fn poll_irq(&mut self) -> bool {
        self.irq
    }
}

#[cfg(test)]
mod tests;
