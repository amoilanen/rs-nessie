//! NES 2C02 picture processing unit.
//!
//! Scope of this implementation:
//!
//! - **Timing**: dot-accurate frame (341 dots × 262 scanlines, with the
//!   odd-frame dot skip when rendering is enabled). The vblank flag is set
//!   on the canonical (scanline 241, dot 1) beat and cleared on
//!   (scanline 261, dot 1). NMI is generated on the rising edge of
//!   `PPUCTRL.7 && PPUSTATUS.7`, matching the standard NES wiring.
//! - **Rendering**: the real background fetch pipeline — nametable /
//!   attribute / pattern fetches every 8 dots into 16-bit shift registers,
//!   with the "loopy" `v`/`t`/`x`/`w` scroll registers updated exactly when
//!   the hardware does (coarse-X increment every 8 dots, fine-Y increment at
//!   dot 256, horizontal `t → v` copy at dot 257, vertical copy on dots
//!   280–304 of the pre-render line). Mid-frame scroll / nametable / pattern
//!   changes (status bars, split screens) therefore render correctly.
//! - **Sprites**: per-scanline evaluation (8-sprite limit, overflow flag),
//!   pattern fetches on dots 257–320 (so MMC3's A12-driven scanline counter
//!   sees the same address pattern as on hardware), 8×8 and 8×16 sprites,
//!   flipping, priority, and dot-accurate sprite-zero hit.
//! - **VRAM / palette / OAM**: 2 KB nametable RAM, 32 B palette RAM with
//!   the standard $3F10/14/18/1C → $3F00/04/08/0C mirrors, and 256 B OAM.
//!   Greyscale (`PPUMASK.0`) is honoured.
//! - **Registers**: `$2007` read buffer, the PPU's I/O data-bus latch
//!   (open bus) for write-only registers and the low bits of `$2002`.
//! - **Output**: the framebuffer is double-buffered; [`Ppu::framebuffer`]
//!   always returns the last *completed* frame.
//! - **Mapper interaction**: the PPU does not own CHR memory. CHR reads and
//!   writes go through the active [`Mapper`]; nametable mirroring also
//!   delegates to [`Mapper::mirroring`] so mapper-driven mode changes
//!   propagate.
//!
//! Not modelled: colour emphasis bits, open-bus decay, sprite-overflow
//! hardware bug, and the exact `$2002` read / vblank-set race suppression.

use crate::cart::Mapper;

pub mod palette;

#[cfg(test)]
mod tests;

/// Width of the NES picture in pixels.
pub const SCREEN_WIDTH: usize = 256;
/// Height of the NES picture in pixels.
pub const SCREEN_HEIGHT: usize = 240;
/// Total framebuffer length in bytes (RGBA8).
pub const FRAMEBUFFER_BYTES: usize = SCREEN_WIDTH * SCREEN_HEIGHT * 4;

/// PPU cycles per scanline.
pub const DOTS_PER_SCANLINE: u16 = 341;
/// Total scanlines per frame (NTSC).
pub const SCANLINES_PER_FRAME: u16 = 262;
/// Scanline at which vblank starts.
pub const VBLANK_START_SCANLINE: u16 = 241;
/// Pre-render scanline index.
pub const PRE_RENDER_SCANLINE: u16 = 261;

// PPUCTRL bit masks.
mod ctrl {
    pub const VRAM_INCREMENT: u8 = 1 << 2;
    pub const SPRITE_PATTERN: u8 = 1 << 3;
    pub const BG_PATTERN: u8 = 1 << 4;
    pub const SPRITE_SIZE: u8 = 1 << 5;
    pub const NMI_ENABLE: u8 = 1 << 7;
}

// PPUMASK bit masks.
mod mask {
    pub const GRAYSCALE: u8 = 1 << 0;
    pub const SHOW_BG_LEFT8: u8 = 1 << 1;
    pub const SHOW_SPR_LEFT8: u8 = 1 << 2;
    pub const SHOW_BG: u8 = 1 << 3;
    pub const SHOW_SPR: u8 = 1 << 4;
}

// PPUSTATUS bit masks.
mod status_bits {
    pub const SPRITE_OVERFLOW: u8 = 1 << 5;
    pub const SPRITE_ZERO_HIT: u8 = 1 << 6;
    pub const VBLANK: u8 = 1 << 7;
}

/// One sprite selected for the scanline being drawn, with its pattern bytes
/// already fetched (and horizontally pre-flipped).
#[derive(Debug, Clone, Copy, Default)]
struct LineSprite {
    /// X coordinate (left of sprite).
    x: u8,
    /// Attribute byte (palette/priority/flip).
    attr: u8,
    /// Pattern plane 0 (bit 7 = leftmost pixel).
    lo: u8,
    /// Pattern plane 1 (bit 7 = leftmost pixel).
    hi: u8,
    /// `true` for OAM entry 0 (sprite-zero hit candidate).
    is_zero: bool,
}

/// Sprite found by evaluation, waiting for its pattern fetch.
#[derive(Debug, Clone, Copy, Default)]
struct EvaluatedSprite {
    y: u8,
    tile: u8,
    attr: u8,
    x: u8,
    is_zero: bool,
}

/// The NES 2C02 picture processing unit state.
pub struct Ppu {
    // === Memory-mapped registers (only the bits the host can see). ===
    ctrl: u8,
    mask: u8,
    status: u8,
    oam_addr: u8,
    /// Last value driven on the PPU's CPU-facing data bus (open bus).
    io_latch: u8,

    // === Internal "loopy" registers. ===
    /// Current VRAM address (15 bits).
    v: u16,
    /// Temporary VRAM address (15 bits) — staged by writes to $2005/$2006.
    t: u16,
    /// Fine X scroll (3 bits).
    x: u8,
    /// Write toggle latch: `false` = first write, `true` = second write.
    w: bool,

    /// $2007 read buffer (palette reads bypass it, all other reads use it).
    data_buffer: u8,

    // === Memory the PPU owns. ===
    /// 2 KB nametable RAM (logical NT0..NT3 after mapper-driven mirroring).
    vram: [u8; 2048],
    /// 32 B palette RAM with the standard mirror at indices 0x10/0x14/0x18/0x1C.
    palette_ram: [u8; 32],
    /// Object attribute memory (64 sprites × 4 bytes).
    oam: [u8; 256],

    // === Background pipeline. ===
    nt_latch: u8,
    at_latch: u8,
    pattern_lo_latch: u8,
    pattern_hi_latch: u8,
    bg_shift_lo: u16,
    bg_shift_hi: u16,
    attr_shift_lo: u16,
    attr_shift_hi: u16,

    // === Sprite pipeline. ===
    /// Sprites found by evaluation for the next scanline.
    eval_sprites: [EvaluatedSprite; 8],
    eval_count: usize,
    /// Sprites (with fetched patterns) being drawn on the current scanline.
    line_sprites: [LineSprite; 8],
    line_sprite_count: usize,
    /// Sprites fetched during dots 257–320, promoted to `line_sprites` at
    /// the start of the next scanline.
    next_line_sprites: [LineSprite; 8],

    // === Frame output. ===
    /// Frame currently being drawn.
    back_buffer: Box<[u8; FRAMEBUFFER_BYTES]>,
    /// Last completed frame (what [`Ppu::framebuffer`] returns).
    front_buffer: Box<[u8; FRAMEBUFFER_BYTES]>,

    // === Timing. ===
    /// Current dot within scanline (0..=340).
    cycle: u16,
    /// Current scanline (0..=261; 261 is pre-render).
    scanline: u16,
    /// Number of frames rendered since power-on.
    frame_count: u64,
    /// Toggles every frame; odd frames skip a dot when rendering.
    odd_frame: bool,

    // === Interrupt line. ===
    /// Edge-detected NMI pulse waiting to be consumed by the CPU bus.
    nmi_pending: bool,
    /// Snapshot of the NMI output line; used to detect the rising edge.
    nmi_previous: bool,
}

impl Default for Ppu {
    fn default() -> Self {
        Self::new()
    }
}

impl Ppu {
    /// Build a freshly powered-on PPU. All registers and memory are zero.
    pub fn new() -> Self {
        Self {
            ctrl: 0,
            mask: 0,
            status: 0,
            oam_addr: 0,
            io_latch: 0,
            v: 0,
            t: 0,
            x: 0,
            w: false,
            data_buffer: 0,
            vram: [0u8; 2048],
            palette_ram: [0u8; 32],
            oam: [0u8; 256],
            nt_latch: 0,
            at_latch: 0,
            pattern_lo_latch: 0,
            pattern_hi_latch: 0,
            bg_shift_lo: 0,
            bg_shift_hi: 0,
            attr_shift_lo: 0,
            attr_shift_hi: 0,
            eval_sprites: [EvaluatedSprite::default(); 8],
            eval_count: 0,
            line_sprites: [LineSprite::default(); 8],
            line_sprite_count: 0,
            next_line_sprites: [LineSprite::default(); 8],
            back_buffer: Box::new([0u8; FRAMEBUFFER_BYTES]),
            front_buffer: Box::new([0u8; FRAMEBUFFER_BYTES]),
            cycle: 0,
            scanline: 0,
            frame_count: 0,
            odd_frame: false,
            nmi_pending: false,
            nmi_previous: false,
        }
    }

    /// Reset (RESET pin asserted) — clears the registers most likely to be
    /// observed in a deterministic state. VRAM/palette/OAM are preserved as
    /// on real hardware.
    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.mask = 0;
        self.w = false;
        self.x = 0;
        self.t = 0;
        self.data_buffer = 0;
        self.cycle = 0;
        self.scanline = 0;
        self.odd_frame = false;
        self.nmi_pending = false;
        self.nmi_previous = false;
    }

    /// Borrow the most recently *completed* frame (RGBA8, row-major,
    /// top-to-bottom).
    #[inline]
    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_BYTES] {
        &self.front_buffer
    }

    /// Number of completed frames since power-on.
    #[inline]
    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }

    /// `(scanline, dot)` of the next tick to run, for diagnostics.
    #[inline]
    pub fn position(&self) -> (u16, u16) {
        (self.scanline, self.cycle)
    }

    /// Take the pending NMI edge, if any. Returns `true` exactly once per
    /// low→high transition of the PPU NMI output line.
    #[inline]
    pub fn take_nmi(&mut self) -> bool {
        let n = self.nmi_pending;
        self.nmi_pending = false;
        n
    }

    /// `true` while the PPU is currently asserting the NMI output line
    /// (`PPUCTRL.NMI_ENABLE && PPUSTATUS.VBLANK`). Provided for tests; the
    /// bus uses [`Ppu::take_nmi`] for actual CPU wiring.
    #[inline]
    pub fn nmi_line(&self) -> bool {
        (self.ctrl & ctrl::NMI_ENABLE != 0) && (self.status & status_bits::VBLANK != 0)
    }

    /// Read a memory-mapped PPU register. `addr` may be any value in
    /// `$2000-$3FFF`; the low 3 bits select the register.
    pub fn read_register(&mut self, addr: u16, mapper: &mut dyn Mapper) -> u8 {
        let value = match addr & 0x7 {
            // $2002 PPUSTATUS — clears vblank and the write toggle on read.
            // The low five bits are not driven and read back as open bus.
            2 => {
                let value = (self.status & 0xE0) | (self.io_latch & 0x1F);
                self.status &= !status_bits::VBLANK;
                self.w = false;
                self.refresh_nmi_edge();
                value
            }
            // $2004 OAMDATA — direct OAM read, does not increment OAMADDR.
            // Bits 2-4 of attribute bytes do not exist and read back as 0.
            4 => {
                let v = self.oam[self.oam_addr as usize];
                if self.oam_addr & 0x03 == 0x02 {
                    v & 0xE3
                } else {
                    v
                }
            }
            // $2007 PPUDATA — buffered read except for palette range.
            7 => {
                let v_before = self.v & 0x3FFF;
                let value = if v_before >= 0x3F00 {
                    // Palette reads return the value immediately (upper two
                    // bits are open bus); the buffer is refreshed from the
                    // nametable "underneath" the palette.
                    let palette_byte =
                        (self.read_palette(v_before) & 0x3F) | (self.io_latch & 0xC0);
                    self.data_buffer = self.read_vram_byte(v_before - 0x1000, mapper);
                    palette_byte
                } else {
                    let buffered = self.data_buffer;
                    self.data_buffer = self.read_vram_byte(v_before, mapper);
                    buffered
                };
                self.increment_v_after_data_access(mapper);
                value
            }
            // Write-only registers return the open-bus latch.
            _ => self.io_latch,
        };
        self.io_latch = value;
        value
    }

    /// Write a memory-mapped PPU register.
    pub fn write_register(&mut self, addr: u16, value: u8, mapper: &mut dyn Mapper) {
        self.io_latch = value;
        match addr & 0x7 {
            // $2000 PPUCTRL.
            0 => {
                self.ctrl = value;
                // t: ....BA.. ........ = d: ......BA
                self.t = (self.t & 0xF3FF) | ((u16::from(value) & 0x03) << 10);
                // Setting NMI_ENABLE while vblank is currently asserted
                // triggers an NMI.
                self.refresh_nmi_edge();
            }
            // $2001 PPUMASK.
            1 => self.mask = value,
            // $2002 PPUSTATUS is read-only; writes are ignored.
            2 => {}
            // $2003 OAMADDR.
            3 => self.oam_addr = value,
            // $2004 OAMDATA — writes increment OAMADDR.
            4 => {
                self.oam[self.oam_addr as usize] = value;
                self.oam_addr = self.oam_addr.wrapping_add(1);
            }
            // $2005 PPUSCROLL — two writes: first X, then Y.
            5 => {
                if !self.w {
                    // First write: coarse X into bits 0-4, fine X into self.x.
                    self.t = (self.t & 0xFFE0) | (u16::from(value) >> 3);
                    self.x = value & 0x07;
                    self.w = true;
                } else {
                    // Second write: coarse Y into bits 5-9, fine Y into 12-14.
                    let coarse_y = u16::from(value) >> 3;
                    let fine_y = u16::from(value) & 0x07;
                    self.t = (self.t & 0x8C1F) | (coarse_y << 5) | (fine_y << 12);
                    self.w = false;
                }
            }
            // $2006 PPUADDR — two writes: high byte, then low byte.
            6 => {
                if !self.w {
                    // t: ..FEDCBA ........ = d: ..FEDCBA;  t: X....... = 0
                    self.t = (self.t & 0x00FF) | ((u16::from(value) & 0x3F) << 8);
                    self.w = true;
                } else {
                    // t: ........ HGFEDCBA = d: HGFEDCBA;  v = t
                    self.t = (self.t & 0xFF00) | u16::from(value);
                    self.v = self.t;
                    self.w = false;
                    mapper.ppu_address_changed(self.v & 0x3FFF);
                }
            }
            // $2007 PPUDATA.
            7 => {
                let addr = self.v & 0x3FFF;
                self.write_vram_byte(addr, value, mapper);
                self.increment_v_after_data_access(mapper);
            }
            _ => unreachable!(),
        }
    }

    /// Receive a 256-byte OAM DMA block. Equivalent to 256 OAMDATA writes
    /// starting at the current `oam_addr`.
    pub fn oam_dma_write(&mut self, page: &[u8; 256]) {
        for (i, byte) in page.iter().enumerate() {
            let addr = self.oam_addr.wrapping_add(i as u8) as usize;
            self.oam[addr] = *byte;
        }
    }

    /// Advance the PPU by `cpu_cycles` CPU cycles (three PPU dots each on
    /// NTSC silicon), rendering pixels and updating timing flags as it goes.
    pub fn step(&mut self, cpu_cycles: u32, mapper: &mut dyn Mapper) {
        let ppu_cycles = cpu_cycles.saturating_mul(3);
        for _ in 0..ppu_cycles {
            self.tick_one(mapper);
        }
    }

    // -----------------------------------------------------------------------
    // Internal: per-dot state machine.
    // -----------------------------------------------------------------------

    fn tick_one(&mut self, mapper: &mut dyn Mapper) {
        let scanline = self.scanline;
        let dot = self.cycle;
        let visible_line = scanline < SCREEN_HEIGHT as u16;
        let pre_render = scanline == PRE_RENDER_SCANLINE;
        let rendering = self.rendering_enabled();

        if pre_render && dot == 1 {
            self.status &= !(status_bits::VBLANK
                | status_bits::SPRITE_ZERO_HIT
                | status_bits::SPRITE_OVERFLOW);
            self.refresh_nmi_edge();
        }
        if scanline == VBLANK_START_SCANLINE && dot == 1 {
            self.status |= status_bits::VBLANK;
            self.refresh_nmi_edge();
            // The visible picture is complete: publish it.
            std::mem::swap(&mut self.front_buffer, &mut self.back_buffer);
        }

        // The fetch pipeline runs first so the pixel for this dot is taken
        // from the shift registers *after* this dot's shift.
        if rendering && (visible_line || pre_render) {
            self.run_fetch_pipeline(dot, pre_render, mapper);
        }

        if visible_line && (1..=256).contains(&dot) {
            self.output_pixel(dot - 1);
        }

        // Advance the dot/scanline/frame counters.
        self.cycle += 1;
        // Odd frames are one dot shorter when rendering is enabled: the
        // pre-render line's last dot is skipped.
        if pre_render && self.cycle == 340 && self.odd_frame && rendering {
            self.cycle = DOTS_PER_SCANLINE;
        }
        if self.cycle >= DOTS_PER_SCANLINE {
            self.cycle = 0;
            self.scanline += 1;
            if self.scanline < SCREEN_HEIGHT as u16 {
                self.start_scanline_sprites();
            }
            if self.scanline >= SCANLINES_PER_FRAME {
                self.scanline = 0;
                self.frame_count = self.frame_count.wrapping_add(1);
                self.odd_frame = !self.odd_frame;
                self.start_scanline_sprites();
            }
        }
    }

    /// Background + sprite memory fetches and loopy-register updates for one
    /// dot of a rendering scanline (visible or pre-render).
    fn run_fetch_pipeline(&mut self, dot: u16, pre_render: bool, mapper: &mut dyn Mapper) {
        // Background fetches: dots 1-256 for the current line and 321-336
        // for the first two tiles of the next line.
        if (2..=257).contains(&dot) || (322..=337).contains(&dot) {
            self.shift_background();
        }
        if (1..=256).contains(&dot) || (321..=336).contains(&dot) {
            match dot & 0x07 {
                1 => {
                    self.reload_background_shifters();
                    self.nt_latch = self.read_nametable(0x2000 | (self.v & 0x0FFF), mapper);
                }
                3 => {
                    let v = self.v;
                    let at_addr = 0x23C0 | (v & 0x0C00) | ((v >> 4) & 0x38) | ((v >> 2) & 0x07);
                    let at = self.read_nametable(at_addr, mapper);
                    let shift = ((v >> 4) & 0x04) | (v & 0x02);
                    self.at_latch = (at >> shift) & 0x03;
                }
                5 => {
                    let addr = self.bg_pattern_addr();
                    self.pattern_lo_latch = mapper.ppu_read(addr);
                }
                7 => {
                    let addr = self.bg_pattern_addr() + 8;
                    self.pattern_hi_latch = mapper.ppu_read(addr);
                }
                0 => self.increment_coarse_x(),
                _ => {}
            }
        }
        if dot == 256 {
            self.increment_y();
        }
        if dot == 257 {
            self.reload_background_shifters();
            // Horizontal bits of t → v.
            self.v = (self.v & !0x041F) | (self.t & 0x041F);
            if pre_render {
                self.eval_count = 0;
            } else {
                self.evaluate_sprites();
            }
        }
        if pre_render && (280..=304).contains(&dot) {
            // Vertical bits of t → v.
            self.v = (self.v & !0x7BE0) | (self.t & 0x7BE0);
        }
        // Sprite pattern fetches: 8 slots of 8 dots across 257-320. The
        // pattern bytes are read on the 5th and 7th dot of each slot.
        if (257..=320).contains(&dot) {
            let slot = ((dot - 257) / 8) as usize;
            match (dot - 257) % 8 {
                4 => self.fetch_sprite_pattern(slot, false, mapper),
                6 => self.fetch_sprite_pattern(slot, true, mapper),
                _ => {}
            }
        }
        // Unused nametable fetches at the end of the line (MMC5 and some
        // A12 watchers care).
        if dot == 337 || dot == 339 {
            let _ = self.read_nametable(0x2000 | (self.v & 0x0FFF), mapper);
        }
    }

    /// Recompute the NMI line and latch the rising edge into `nmi_pending`.
    fn refresh_nmi_edge(&mut self) {
        let now = self.nmi_line();
        if now && !self.nmi_previous {
            self.nmi_pending = true;
        }
        self.nmi_previous = now;
    }

    /// True when either background or sprite rendering is enabled.
    #[inline]
    fn rendering_enabled(&self) -> bool {
        self.mask & (mask::SHOW_BG | mask::SHOW_SPR) != 0
    }

    /// `$2007` accesses use the increment-mode bit of PPUCTRL.
    #[inline]
    fn vram_increment(&self) -> u16 {
        if self.ctrl & ctrl::VRAM_INCREMENT != 0 {
            32
        } else {
            1
        }
    }

    fn increment_v_after_data_access(&mut self, mapper: &mut dyn Mapper) {
        self.v = self.v.wrapping_add(self.vram_increment()) & 0x7FFF;
        // Outside of rendering the PPU keeps `v` on its address bus.
        mapper.ppu_address_changed(self.v & 0x3FFF);
    }

    // -----------------------------------------------------------------------
    // Loopy register helpers.
    // -----------------------------------------------------------------------

    fn increment_coarse_x(&mut self) {
        if self.v & 0x001F == 31 {
            self.v &= !0x001F;
            self.v ^= 0x0400; // switch horizontal nametable
        } else {
            self.v += 1;
        }
    }

    fn increment_y(&mut self) {
        if self.v & 0x7000 != 0x7000 {
            self.v += 0x1000; // fine Y
        } else {
            self.v &= !0x7000;
            let mut coarse_y = (self.v & 0x03E0) >> 5;
            if coarse_y == 29 {
                coarse_y = 0;
                self.v ^= 0x0800; // switch vertical nametable
            } else if coarse_y == 31 {
                coarse_y = 0; // out-of-range rows wrap without switching
            } else {
                coarse_y += 1;
            }
            self.v = (self.v & !0x03E0) | (coarse_y << 5);
        }
    }

    #[inline]
    fn bg_pattern_addr(&self) -> u16 {
        let base = if self.ctrl & ctrl::BG_PATTERN != 0 {
            0x1000
        } else {
            0x0000
        };
        let fine_y = (self.v >> 12) & 0x07;
        base + u16::from(self.nt_latch) * 16 + fine_y
    }

    fn shift_background(&mut self) {
        self.bg_shift_lo <<= 1;
        self.bg_shift_hi <<= 1;
        self.attr_shift_lo <<= 1;
        self.attr_shift_hi <<= 1;
    }

    fn reload_background_shifters(&mut self) {
        self.bg_shift_lo = (self.bg_shift_lo & 0xFF00) | u16::from(self.pattern_lo_latch);
        self.bg_shift_hi = (self.bg_shift_hi & 0xFF00) | u16::from(self.pattern_hi_latch);
        let lo = if self.at_latch & 0x01 != 0 {
            0xFF
        } else {
            0x00
        };
        let hi = if self.at_latch & 0x02 != 0 {
            0xFF
        } else {
            0x00
        };
        self.attr_shift_lo = (self.attr_shift_lo & 0xFF00) | lo;
        self.attr_shift_hi = (self.attr_shift_hi & 0xFF00) | hi;
    }

    // -----------------------------------------------------------------------
    // VRAM / palette helpers.
    // -----------------------------------------------------------------------

    /// Resolve a `$2000-$2FFF` (or `$3000-$3EFF` mirror) PPU address into a
    /// linear index into the 2 KB nametable RAM, honouring the cartridge's
    /// current mirroring.
    fn nametable_index(&self, addr: u16, mapper: &dyn Mapper) -> usize {
        let table = mapper.mirroring().nametable_index(addr) & 1;
        let offset = (addr & 0x03FF) as usize;
        table * 0x0400 + offset
    }

    /// Resolve `$3F00-$3FFF` into the 32-byte palette RAM with the
    /// well-known mirrors at `$3F10/14/18/1C → $3F00/04/08/0C`.
    #[inline]
    fn palette_index(addr: u16) -> usize {
        let mut idx = (addr as usize) & 0x1F;
        if idx & 0x13 == 0x10 {
            // $3F10/14/18/1C → mirror down by clearing bit 4.
            idx &= 0x0F;
        }
        idx
    }

    fn read_palette(&self, addr: u16) -> u8 {
        self.palette_ram[Self::palette_index(addr)]
    }

    fn write_palette(&mut self, addr: u16, value: u8) {
        self.palette_ram[Self::palette_index(addr)] = value & 0x3F;
    }

    fn read_vram_byte(&self, addr: u16, mapper: &mut dyn Mapper) -> u8 {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => mapper.ppu_read(a),
            0x2000..=0x3EFF => self.vram[self.nametable_index(a, mapper)],
            0x3F00..=0x3FFF => self.read_palette(a),
            _ => unreachable!(),
        }
    }

    /// Read a nametable byte during rendering without notifying the mapper
    /// of the address (keeps the per-dot fetch path cheap; no supported
    /// mapper watches nametable fetches).
    #[inline]
    fn read_nametable(&self, addr: u16, mapper: &dyn Mapper) -> u8 {
        self.vram[self.nametable_index(addr, mapper)]
    }

    fn write_vram_byte(&mut self, addr: u16, value: u8, mapper: &mut dyn Mapper) {
        let a = addr & 0x3FFF;
        match a {
            0x0000..=0x1FFF => mapper.ppu_write(a, value),
            0x2000..=0x3EFF => {
                let idx = self.nametable_index(a, mapper);
                self.vram[idx] = value;
            }
            0x3F00..=0x3FFF => self.write_palette(a, value),
            _ => unreachable!(),
        }
    }

    // -----------------------------------------------------------------------
    // Sprites.
    // -----------------------------------------------------------------------

    #[inline]
    fn sprite_height(&self) -> u16 {
        if self.ctrl & ctrl::SPRITE_SIZE != 0 {
            16
        } else {
            8
        }
    }

    /// Select the (up to 8) sprites that intersect the *next* scanline. A
    /// sprite with OAM Y coordinate `y` covers screen rows `y+1 ..= y+h`, so
    /// evaluating against the current scanline number yields the sprites for
    /// the following line.
    fn evaluate_sprites(&mut self) {
        self.eval_count = 0;
        let line = self.scanline;
        let height = self.sprite_height();
        for i in 0..64 {
            let base = i * 4;
            let y = self.oam[base];
            let row = line.wrapping_sub(u16::from(y));
            if row >= height {
                continue;
            }
            if self.eval_count == 8 {
                self.status |= status_bits::SPRITE_OVERFLOW;
                break;
            }
            self.eval_sprites[self.eval_count] = EvaluatedSprite {
                y,
                tile: self.oam[base + 1],
                attr: self.oam[base + 2],
                x: self.oam[base + 3],
                is_zero: i == 0,
            };
            self.eval_count += 1;
        }
    }

    /// Fetch one pattern plane for sprite slot `slot` (dots 257–320). Empty
    /// slots still fetch tile `$FF`, as the hardware does, so mapper A12
    /// watchers see a realistic address stream.
    fn fetch_sprite_pattern(&mut self, slot: usize, high_plane: bool, mapper: &mut dyn Mapper) {
        let height = self.sprite_height();
        let (sprite, present) = if slot < self.eval_count {
            (self.eval_sprites[slot], true)
        } else {
            (
                EvaluatedSprite {
                    y: 0xFF,
                    tile: 0xFF,
                    attr: 0xFF,
                    x: 0xFF,
                    is_zero: false,
                },
                false,
            )
        };
        let mut row = if present {
            self.scanline.wrapping_sub(u16::from(sprite.y)) & (height - 1)
        } else {
            0
        };
        if present && sprite.attr & 0x80 != 0 {
            row = height - 1 - row; // vertical flip
        }
        let addr = if height == 16 {
            let table = if sprite.tile & 0x01 != 0 {
                0x1000
            } else {
                0x0000
            };
            let tile = u16::from(sprite.tile & 0xFE) + u16::from(row >= 8);
            table + tile * 16 + (row & 0x07)
        } else {
            let table = if self.ctrl & ctrl::SPRITE_PATTERN != 0 {
                0x1000
            } else {
                0x0000
            };
            table + u16::from(sprite.tile) * 16 + row
        };
        let byte = mapper.ppu_read(if high_plane { addr + 8 } else { addr });
        if !present {
            return;
        }
        let byte = if sprite.attr & 0x40 != 0 {
            byte.reverse_bits() // horizontal flip
        } else {
            byte
        };
        let entry = &mut self.next_line_sprites[slot];
        entry.x = sprite.x;
        entry.attr = sprite.attr;
        entry.is_zero = sprite.is_zero;
        if high_plane {
            entry.hi = byte;
        } else {
            entry.lo = byte;
        }
    }

    /// Promote the sprites fetched during the previous line to the line about
    /// to be drawn.
    fn start_scanline_sprites(&mut self) {
        if self.rendering_enabled() && self.scanline != 0 {
            self.line_sprites = self.next_line_sprites;
            self.line_sprite_count = self.eval_count;
        } else {
            // Sprites never appear on scanline 0 (no evaluation happens on
            // the pre-render line), nor when rendering is off.
            self.line_sprite_count = 0;
        }
    }

    // -----------------------------------------------------------------------
    // Pixel output.
    // -----------------------------------------------------------------------

    fn output_pixel(&mut self, x: u16) {
        let xi = x as usize;
        let show_bg =
            self.mask & mask::SHOW_BG != 0 && (xi >= 8 || self.mask & mask::SHOW_BG_LEFT8 != 0);
        let show_spr =
            self.mask & mask::SHOW_SPR != 0 && (xi >= 8 || self.mask & mask::SHOW_SPR_LEFT8 != 0);

        // ----- Background pixel from the shift registers.
        let (bg_pixel, bg_palette) = if show_bg {
            let bit = 15 - u16::from(self.x);
            let p0 = ((self.bg_shift_lo >> bit) & 1) as u8;
            let p1 = ((self.bg_shift_hi >> bit) & 1) as u8;
            let a0 = ((self.attr_shift_lo >> bit) & 1) as u8;
            let a1 = ((self.attr_shift_hi >> bit) & 1) as u8;
            ((p1 << 1) | p0, (a1 << 1) | a0)
        } else {
            (0, 0)
        };

        // ----- Sprite pixel: first opaque sprite in OAM order wins.
        let mut spr_pixel = 0u8;
        let mut spr_palette = 0u8;
        let mut spr_front = false;
        let mut spr_zero = false;
        if show_spr {
            for s in &self.line_sprites[..self.line_sprite_count] {
                let offset = x.wrapping_sub(u16::from(s.x));
                if offset >= 8 {
                    continue;
                }
                let bit = 7 - offset;
                let p = (((s.hi >> bit) & 1) << 1) | ((s.lo >> bit) & 1);
                if p != 0 {
                    spr_pixel = p;
                    spr_palette = s.attr & 0x03;
                    spr_front = s.attr & 0x20 == 0;
                    spr_zero = s.is_zero;
                    break;
                }
            }
        }

        // ----- Sprite-zero hit: both opaque, not at x=255. The left-8
        // clipping is already folded into `show_bg`/`show_spr`.
        if spr_zero && bg_pixel != 0 && spr_pixel != 0 && xi != 255 {
            self.status |= status_bits::SPRITE_ZERO_HIT;
        }

        // ----- Priority multiplexer.
        let palette_addr = match (bg_pixel, spr_pixel) {
            (0, 0) => 0x3F00,
            (0, _) => 0x3F10 | (u16::from(spr_palette) << 2) | u16::from(spr_pixel),
            (_, 0) => 0x3F00 | (u16::from(bg_palette) << 2) | u16::from(bg_pixel),
            _ if spr_front => 0x3F10 | (u16::from(spr_palette) << 2) | u16::from(spr_pixel),
            _ => 0x3F00 | (u16::from(bg_palette) << 2) | u16::from(bg_pixel),
        };

        // With rendering disabled the PPU outputs the backdrop colour —
        // or, if `v` points into palette RAM, that palette entry (the
        // "background palette hack" some demos use).
        let palette_addr = if !self.rendering_enabled() && self.v & 0x3F00 == 0x3F00 {
            self.v
        } else {
            palette_addr
        };
        let mut color_index = self.read_palette(palette_addr) & 0x3F;
        if self.mask & mask::GRAYSCALE != 0 {
            color_index &= 0x30;
        }
        let color = palette::NES_PALETTE[color_index as usize];
        let off = (self.scanline as usize * SCREEN_WIDTH + xi) * 4;
        let px = &mut self.back_buffer[off..off + 4];
        px[0] = color[0];
        px[1] = color[1];
        px[2] = color[2];
        px[3] = 0xFF;
    }

    // -----------------------------------------------------------------------
    // Diagnostic helpers (used by tests).
    // -----------------------------------------------------------------------

    /// Read the raw byte at PPU address `addr` (no buffer, no v mutation).
    #[doc(hidden)]
    pub fn debug_peek(&self, addr: u16, mapper: &mut dyn Mapper) -> u8 {
        self.read_vram_byte(addr, mapper)
    }

    /// Internal `v` register (15-bit).
    #[doc(hidden)]
    pub fn debug_v(&self) -> u16 {
        self.v
    }

    /// Internal `t` register (15-bit).
    #[doc(hidden)]
    pub fn debug_t(&self) -> u16 {
        self.t
    }

    /// Internal fine-X (3-bit).
    #[doc(hidden)]
    pub fn debug_x(&self) -> u8 {
        self.x
    }

    /// Internal write-toggle latch.
    #[doc(hidden)]
    pub fn debug_w(&self) -> bool {
        self.w
    }

    /// Mutable OAM access for OAM DMA testing.
    #[doc(hidden)]
    pub fn debug_oam_mut(&mut self) -> &mut [u8; 256] {
        &mut self.oam
    }
}

// Re-export the bits that are part of the public API surface so callers
// don't have to reach into private modules to test individual flags.
pub use status_bits::{SPRITE_OVERFLOW, SPRITE_ZERO_HIT, VBLANK};
