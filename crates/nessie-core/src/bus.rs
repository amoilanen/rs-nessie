//! NES CPU bus implementation.
//!
//! Implements the canonical NES CPU memory map for the [`crate::cpu::CpuBus`]
//! trait:
//!
//! | Range           | Device                                                |
//! |-----------------|-------------------------------------------------------|
//! | `$0000-$07FF`   | 2 KB internal CPU RAM                                 |
//! | `$0800-$1FFF`   | Mirrors of `$0000-$07FF` (mask `$07FF`)               |
//! | `$2000-$2007`   | PPU registers                                         |
//! | `$2008-$3FFF`   | Mirrors of `$2000-$2007` (mask `$0007`)               |
//! | `$4000-$4015`   | APU registers                                         |
//! | `$4016`         | Controller 1                                          |
//! | `$4017`         | Controller 2 / APU frame counter                      |
//! | `$4018-$401F`   | APU/IO test mode (open bus on commercial hardware)    |
//! | `$4020-$FFFF`   | Cartridge space, routed through the [`crate::cart::Mapper`] |
//!
//! `NesBus` is a **borrowed view** over the underlying components. The
//! owning [`crate::Nes`] facade keeps the CPU RAM, PPU, APU, cartridge,
//! controller pair, and interrupt latches as separate fields and assembles
//! a `NesBus` per CPU step so the borrow checker accepts shared access to
//! peripherals (e.g. the PPU's register handler needs `&mut Mapper`).

use crate::apu::Apu;
use crate::cart::{Cartridge, Mapper};
use crate::controller::Controller;
use crate::cpu::CpuBus;
use crate::ppu::Ppu;

/// The default size of the internal CPU RAM.
pub const CPU_RAM_SIZE: usize = 0x0800;

/// A borrowed view that implements [`CpuBus`] over the NES's CPU-side
/// peripherals.
///
/// Build one on the stack for each CPU step from disjoint fields of the
/// owning [`crate::Nes`] (or, in tests, from local variables).
pub struct NesBus<'a> {
    /// 2 KB internal CPU work RAM.
    pub ram: &'a mut [u8; CPU_RAM_SIZE],
    /// Cartridge mapper that owns PRG/CHR memory.
    pub cart: &'a mut Cartridge,
    /// Picture processing unit.
    pub ppu: &'a mut Ppu,
    /// Audio processing unit.
    pub apu: &'a mut Apu,
    /// Player 1 and player 2 controllers, in port order.
    pub controllers: &'a mut [Controller; 2],
    /// Pending NMI line state (set by the PPU when entering vblank).
    /// Edge-triggered: cleared by `poll_nmi`.
    pub nmi_pending: &'a mut bool,
    /// Pending IRQ line state (set by host-side overrides; OR-ed with APU
    /// and mapper IRQ inside `poll_irq`).
    pub irq_pending: &'a mut bool,
    /// Extra CPU cycles consumed by side-effects observed during the
    /// current CPU step but not accounted for in the instruction's nominal
    /// cycle count (currently: the 513-cycle OAM DMA stall on writes to
    /// `$4014`). The owning [`crate::Nes`] facade reads and clears this
    /// after each `Cpu::step` so the PPU/APU advance by the right number of
    /// cycles.
    pub extra_cycles: &'a mut u32,
}

impl<'a> NesBus<'a> {
    /// Mutable view of the cartridge's mapper (test/diagnostic use).
    pub fn mapper(&mut self) -> &mut dyn Mapper {
        self.cart.mapper_mut()
    }
}

impl CpuBus for NesBus<'_> {
    fn read(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x1FFF => {
                let idx = (addr as usize) & 0x07FF;
                self.ram[idx]
            }
            0x2000..=0x3FFF => self.ppu.read_register(addr, self.cart.mapper_mut()),
            0x4015 => self.apu.read_status(),
            0x4000..=0x4014 => 0, // APU write-only registers read as open bus (0).
            0x4016 => self.controllers[0].read() & 0x01,
            0x4017 => self.controllers[1].read() & 0x01,
            0x4018..=0x401F => 0, // Test-mode registers, open bus on consumer NES.
            0x4020..=0xFFFF => self.cart.mapper_mut().cpu_read(addr),
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => {
                let idx = (addr as usize) & 0x07FF;
                self.ram[idx] = value;
            }
            0x2000..=0x3FFF => self.ppu.write_register(addr, value, self.cart.mapper_mut()),
            0x4000..=0x4013 | 0x4015 | 0x4017 => self.apu.write_register(addr, value),
            0x4014 => self.do_oam_dma(value),
            0x4016 => {
                // Strobe writes are mirrored to both controllers per the
                // canonical NES wiring (both shift registers share the strobe).
                self.controllers[0].write_strobe(value);
                self.controllers[1].write_strobe(value);
            }
            0x4018..=0x401F => { /* Open bus on consumer NES */ }
            0x4020..=0xFFFF => self.cart.mapper_mut().cpu_write(addr, value),
        }
    }

    fn poll_nmi(&mut self) -> bool {
        // NMI is edge-triggered: consume the pulse on read.
        let n = *self.nmi_pending;
        *self.nmi_pending = false;
        n
    }

    fn poll_irq(&mut self) -> bool {
        // IRQ is level-triggered: combine bus-level flag with APU and
        // mapper IRQ.
        *self.irq_pending || self.apu.irq_pending() || self.cart.mapper().irq_pending()
    }
}

impl NesBus<'_> {
    /// Service a write to `$4014` (OAM DMA): copy 256 bytes from CPU page
    /// `value` into the PPU's OAM. The real CPU stalls for 513 or 514
    /// cycles; the [`crate::Nes`] facade is responsible for accounting for
    /// the extra cycles when ticking the PPU/APU after the write completes.
    fn do_oam_dma(&mut self, page: u8) {
        let base = u16::from(page) << 8;
        let mut block = [0u8; 256];
        for (i, slot) in block.iter_mut().enumerate() {
            // Re-enter our own read path so RAM mirroring, PPU side-effects
            // and mapper PRG-RAM all stay consistent. DMA from PPU/APU
            // registers is undefined on real hardware but harmless here.
            let addr = base.wrapping_add(i as u16);
            *slot = match addr {
                0x0000..=0x1FFF => self.ram[(addr as usize) & 0x07FF],
                0x4020..=0xFFFF => self.cart.mapper_mut().cpu_read(addr),
                // Reading PPU/APU registers during DMA is undefined; pretend
                // open bus.
                _ => 0,
            };
        }
        self.ppu.oam_dma_write(&block);
        // Real hardware stalls the CPU for 513 cycles (514 on odd cycles)
        // while OAM DMA runs. Accumulate them so the facade can tick the
        // PPU/APU through this window.
        *self.extra_cycles = self.extra_cycles.saturating_add(513);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::cart::parse_ines;

    fn dummy_cart() -> Cartridge {
        // 16 KB PRG + 8 KB CHR NROM with byte 0 at $8000 = 0xAB and $C000 = 0xCD.
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1); // PRG units (16 KB)
        rom.push(1); // CHR units (8 KB)
        rom.push(0); // flags6 — mapper 0, horizontal mirroring
        rom.push(0); // flags7
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        rom[16] = 0xAB;
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        parse_ines(&rom).unwrap()
    }

    /// Helper to allocate the disjoint owners that back a `NesBus` for tests.
    struct BusOwners {
        ram: [u8; CPU_RAM_SIZE],
        ppu: Ppu,
        apu: Apu,
        controllers: [Controller; 2],
        nmi: bool,
        irq: bool,
        extra: u32,
    }

    impl BusOwners {
        fn new() -> Self {
            Self {
                ram: [0u8; CPU_RAM_SIZE],
                ppu: Ppu::new(),
                apu: Apu::new(),
                controllers: [Controller::new(); 2],
                nmi: false,
                irq: false,
                extra: 0,
            }
        }
        fn bus<'a>(&'a mut self, cart: &'a mut Cartridge) -> NesBus<'a> {
            NesBus {
                ram: &mut self.ram,
                cart,
                ppu: &mut self.ppu,
                apu: &mut self.apu,
                controllers: &mut self.controllers,
                nmi_pending: &mut self.nmi,
                irq_pending: &mut self.irq,
                extra_cycles: &mut self.extra,
            }
        }
    }

    #[test]
    fn ram_mirrors_on_read_and_write() {
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        let mut bus = owners.bus(&mut cart);
        bus.write(0x0000, 0x42);
        assert_eq!(bus.read(0x0800), 0x42);
        assert_eq!(bus.read(0x1000), 0x42);
        assert_eq!(bus.read(0x1800), 0x42);

        bus.write(0x07FF, 0x99);
        assert_eq!(bus.read(0x0FFF), 0x99);
        assert_eq!(bus.read(0x17FF), 0x99);
        assert_eq!(bus.read(0x1FFF), 0x99);
    }

    #[test]
    fn ppu_apu_and_open_bus_ranges_default_to_zero() {
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        let mut bus = owners.bus(&mut cart);
        for addr in [0x2000u16, 0x2007, 0x3FFF, 0x4000, 0x4015, 0x401F] {
            assert_eq!(bus.read(addr), 0, "addr ${:04X}", addr);
            // Writes must not panic and must not affect RAM mirroring.
            bus.write(addr, 0xFF);
        }
        assert_eq!(bus.read(0x0000), 0);
    }

    #[test]
    fn cartridge_routes_at_8000_plus() {
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        let mut bus = owners.bus(&mut cart);
        // Byte 0 of PRG is 0xAB, mirrored at $C000 for a 16 KB cart.
        assert_eq!(bus.read(0x8000), 0xAB);
        assert_eq!(bus.read(0xC000), 0xAB);
    }

    #[test]
    fn nmi_is_edge_triggered() {
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        owners.nmi = true;
        let mut bus = owners.bus(&mut cart);
        assert!(bus.poll_nmi());
        // Subsequent reads return false until the line is reasserted.
        assert!(!bus.poll_nmi());
        assert!(!bus.poll_nmi());
    }

    #[test]
    fn irq_is_level_triggered() {
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        owners.irq = true;
        let mut bus = owners.bus(&mut cart);
        assert!(bus.poll_irq());
        assert!(bus.poll_irq());
        *bus.irq_pending = false;
        assert!(!bus.poll_irq());
    }

    #[test]
    fn controller_strobe_and_read_routed_through_bus() {
        use crate::controller::Button;
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        owners.controllers[0].set_button(Button::A, true);
        owners.controllers[0].set_button(Button::Start, true);
        owners.controllers[1].set_button(Button::B, true);
        let mut bus = owners.bus(&mut cart);
        bus.write(0x4016, 1);
        bus.write(0x4016, 0);
        // P1: A=1, B=0, Select=0, Start=1, …
        let p1: Vec<u8> = (0..8).map(|_| bus.read(0x4016)).collect();
        assert_eq!(p1, vec![1, 0, 0, 1, 0, 0, 0, 0]);
        // P2: A=0, B=1, …
        let p2: Vec<u8> = (0..8).map(|_| bus.read(0x4017)).collect();
        assert_eq!(p2, vec![0, 1, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn oam_dma_copies_cpu_page_into_ppu_oam() {
        let mut cart = dummy_cart();
        let mut owners = BusOwners::new();
        // Fill CPU page 2 ($0200..$02FF) with a recognizable pattern.
        for i in 0..256u16 {
            owners.ram[(0x0200 + i) as usize & 0x07FF] = (i & 0xFF) as u8;
        }
        let mut bus = owners.bus(&mut cart);
        bus.write(0x4014, 0x02); // Trigger DMA from page $0200.
                                 // Inspect OAM through PPU OAMDATA: write OAMADDR=0, read OAMDATA via
                                 // the PPU's direct accessor.
        bus.write(0x2003, 0); // OAMADDR = 0
        for i in 0u16..256 {
            let i8 = i as u8;
            assert_eq!(bus.read(0x2004), i8, "oam[{}]", i);
            // OAMDATA reads on real hardware leave OAMADDR unchanged; we
            // explicitly bump it to walk through OAM.
            bus.write(0x2003, i8.wrapping_add(1));
        }
    }
}
