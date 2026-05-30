//! `nessie-core` is the engine-agnostic NES emulation core.
//!
//! It hosts the CPU, PPU, APU, cartridge / mapper implementations, the
//! standard controllers, and the public [`Nes`] facade. The crate has zero
//! runtime, audio, or graphics dependencies so it can be embedded by any
//! host (Tauri, headless tests, benches, future WASM target).
//!
//! The public surface intentionally matches the contract in
//! `./.zenflow/tasks/create-a-nes-8bit-emulator-which-2e40/spec.md` §5.4:
//! [`Nes::from_ines`], [`Nes::step_frame`], [`Nes::framebuffer`],
//! [`Nes::drain_audio`], [`Nes::set_button`], [`Nes::reset`],
//! [`Nes::load_battery`], [`Nes::battery_snapshot`], [`Nes::has_battery`],
//! and [`Nes::cartridge_info`].

pub mod apu;
pub mod bus;
pub mod cart;
pub mod controller;
pub mod cpu;
pub mod error;
pub mod nes;
pub mod ppu;

pub use apu::Apu;
pub use bus::NesBus;
pub use cart::{parse_ines, Cartridge, CartridgeInfo, Mapper, Mirroring, ParseError};
pub use controller::{Button, Controller, Player};
pub use cpu::{flag, Cpu, CpuBus, Mnemonic, Mode, OpcodeInfo, INSTR_TABLE, OPCODES};
pub use error::CoreError;
pub use nes::Nes;
pub use ppu::Ppu;
