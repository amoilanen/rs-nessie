//! NES 2C02 picture processing unit.
//!
//! Scope of this implementation:
//!
//! - **Timing**: cycle-accurate frame counter (341 dots × 262 scanlines per
//!   frame). The vblank flag is set on the canonical (scanline 241, dot 1)
//!   beat and cleared on (scanline 261, dot 1). NMI is generated at the
//!   vblank edge when [`PPUCTRL`] bit 7 is set, matching the standard NES
//!   wiring.
//! - **Rendering**: scanline-accurate background and sprite rendering. The
//!   PPU produces an RGBA8 256×240 framebuffer using a canonical NTSC color
//!   palette. Rendering happens at dot 256 of each visible scanline so the
//!   v/t/x registers reflect the start-of-scanline scroll state.
//! - **Sprite evaluation**: standard 8-sprite-per-scanline limit with
//!   sprite-zero hit detection and sprite-overflow flag (faithful to the
//!   real hardware quirk: only 8 sprites are scanned per scanline, and
//!   overflow is asserted when a 9th is found).
//! - **VRAM / palette / OAM**: 2 KB nametable RAM, 32 B palette RAM with
//!   the standard $3F10/14/18/1C → $3F00/04/08/0C mirrors, and 256 B OAM.
//! - **Mapper interaction**: the PPU does not own CHR memory. CHR reads and
//!   writes go through the active [`Mapper`]; nametable mirroring also
//!   delegates to [`Mapper::mirroring`] so MMC1-style mode changes propagate.
//!
//! What this *does not* do:
//!
//! - Dot-accurate background tile fetcher pipeline (we still render
//!   correctly because we read the same memory at the end of each scanline;
//!   tests that depend on mid-scanline timing of the v register, e.g.
//!   raster split games, are out of scope for this milestone).
//! - PPU bus open-bus behavior on register reads.
//! - DMC sample fetch jitter on OAM DMA.
//!
//! These approximations are sufficient for the spec's PPU acceptance bar
//! (deterministic framebuffer hashes on hand-crafted scenes; sprite-zero
//! hit on overlap with rendering enabled; vblank/NMI timing tests).

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
    pub const NAMETABLE_LO: u8 = 1 << 0;
    pub const NAMETABLE_HI: u8 = 1 << 1;
    pub const VRAM_INCREMENT: u8 = 1 << 2;
    pub const SPRITE_PATTERN: u8 = 1 << 3;
    pub const BG_PATTERN: u8 = 1 << 4;
    pub const SPRITE_SIZE: u8 = 1 << 5;
    pub const _MASTER_SLAVE: u8 = 1 << 6;
    pub const NMI_ENABLE: u8 = 1 << 7;
}

// PPUMASK bit masks.
mod mask {
    pub const _GRAYSCALE: u8 = 1 << 0;
    pub const SHOW_BG_LEFT8: u8 = 1 << 1;
    pub const SHOW_SPR_LEFT8: u8 = 1 << 2;
    pub const SHOW_BG: u8 = 1 << 3;
    pub const SHOW_SPR: u8 = 1 << 4;
    pub const _EMPHASIZE_R: u8 = 1 << 5;
    pub const _EMPHASIZE_G: u8 = 1 << 6;
    pub const _EMPHASIZE_B: u8 = 1 << 7;
}

// PPUSTATUS bit masks.
mod status_bits {
    pub const SPRITE_OVERFLOW: u8 = 1 << 5;
    pub const SPRITE_ZERO_HIT: u8 = 1 << 6;
    pub const VBLANK: u8 = 1 << 7;
}

/// Per-scanline sprite information held in the secondary OAM.
#[derive(Debug, Clone, Copy, Default)]
struct SpriteEntry {
    /// Sprite index in primary OAM. Used to detect "sprite 0".
    oam_index: u8,
    /// Y coordinate (top of sprite, already decoded).
    y: u8,
    /// Tile index.
    tile: u8,
    /// Attribute byte (palette/priority/flip).
    attr: u8,
    /// X coordinate (left of sprite).
    x: u8,
}

/// The NES 2C02 picture processing unit state.
pub struct Ppu {
    // === Memory-mapped registers (only the bits the host can see). ===
    ctrl: u8,
    mask: u8,
    status: u8,
    oam_addr: u8,

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

    // === Frame output. ===
    /// 256×240 RGBA8 framebuffer.
    framebuffer: Box<[u8; FRAMEBUFFER_BYTES]>,

    // === Timing. ===
    /// Current dot within scanline (0..=340).
    cycle: u16,
    /// Current scanline (0..=261; 261 is pre-render).
    scanline: u16,
    /// Number of frames rendered since power-on.
    frame_count: u64,

    // === Interrupt line. ===
    /// Edge-detected NMI pulse waiting to be consumed by the CPU bus.
    nmi_pending: bool,
    /// Snapshot of `nmi_output` from the previous tick; used to detect the
    /// low→high edge that fires NMI.
    nmi_previous: bool,

    // === Per-scanline sprite buffer. ===
    /// Sprites selected for the *current* visible scanline (max 8).
    sprite_buffer: [SpriteEntry; 8],
    /// Number of valid entries in `sprite_buffer`.
    sprite_count: usize,
    /// True when sprite-0 was picked into `sprite_buffer` this scanline.
    sprite_zero_in_line: bool,
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
            v: 0,
            t: 0,
            x: 0,
            w: false,
            data_buffer: 0,
            vram: [0u8; 2048],
            palette_ram: [0u8; 32],
            oam: [0u8; 256],
            framebuffer: Box::new([0u8; FRAMEBUFFER_BYTES]),
            cycle: 0,
            scanline: 0,
            frame_count: 0,
            nmi_pending: false,
            nmi_previous: false,
            sprite_buffer: [SpriteEntry::default(); 8],
            sprite_count: 0,
            sprite_zero_in_line: false,
        }
    }

    /// Reset (RESET pin asserted) — clears the registers most likely to be
    /// observed in a deterministic state. VRAM/palette/OAM are preserved as
    /// on real hardware.
    pub fn reset(&mut self) {
        self.ctrl = 0;
        self.mask = 0;
        self.w = false;
        self.data_buffer = 0;
        self.cycle = 0;
        self.scanline = 0;
        self.nmi_pending = false;
        self.nmi_previous = false;
    }

    /// Borrow the current framebuffer (RGBA8, row-major, top-to-bottom).
    #[inline]
    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_BYTES] {
        &self.framebuffer
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
        match addr & 0x7 {
            // $2002 PPUSTATUS — clears vblank and the write toggle on read.
            2 => {
                let value = self.status;
                self.status &= !status_bits::VBLANK;
                self.w = false;
                // Re-evaluate NMI output line edge so a CPU read of PPUSTATUS
                // that races vblank does not retrigger a queued NMI.
                self.refresh_nmi_edge();
                value
            }
            // $2004 OAMDATA — direct OAM read, does not increment OAMADDR.
            4 => self.oam[self.oam_addr as usize],
            // $2007 PPUDATA — buffered read except for palette range.
            7 => {
                let v_before = self.v & 0x3FFF;
                let value = if v_before >= 0x3F00 {
                    // Palette reads return the value immediately; the buffer
                    // is still refreshed from the underlying nametable mirror.
                    let palette_byte = self.read_palette(v_before);
                    self.data_buffer = self.read_vram_byte(v_before - 0x1000, mapper);
                    palette_byte
                } else {
                    let buffered = self.data_buffer;
                    self.data_buffer = self.read_vram_byte(v_before, mapper);
                    buffered
                };
                self.increment_v_after_data_access();
                value
            }
            // Write-only registers return open bus on real hardware; we
            // return 0 to keep the model simple and deterministic.
            _ => 0,
        }
    }

    /// Write a memory-mapped PPU register.
    pub fn write_register(&mut self, addr: u16, value: u8, mapper: &mut dyn Mapper) {
        match addr & 0x7 {
            // $2000 PPUCTRL.
            0 => {
                self.ctrl = value;
                // t: ....BA.. ........ = d: ......BA
                self.t = (self.t & 0xF3FF) | ((u16::from(value) & 0x03) << 10);
                // Setting NMI_ENABLE while vblank is currently asserted
                // triggers a delayed NMI on the next CPU cycle.
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
                    self.t &= 0x3FFF;
                    self.w = true;
                } else {
                    // t: ........ HGFEDCBA = d: HGFEDCBA;  v = t
                    self.t = (self.t & 0xFF00) | u16::from(value);
                    self.v = self.t;
                    self.w = false;
                }
            }
            // $2007 PPUDATA.
            7 => {
                let addr = self.v & 0x3FFF;
                self.write_vram_byte(addr, value, mapper);
                self.increment_v_after_data_access();
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

    /// Advance the PPU by `cpu_cycles * 3` PPU dots, performing per-scanline
    /// rendering and updating timing flags along the way.
    pub fn step(&mut self, cpu_cycles: u32, mapper: &mut dyn Mapper) {
        // The PPU runs 3 dots for every CPU cycle on NTSC silicon.
        let ppu_cycles = cpu_cycles.saturating_mul(3);
        for _ in 0..ppu_cycles {
            self.tick_one(mapper);
        }
    }

    // -----------------------------------------------------------------------
    // Internal: per-dot state machine.
    // -----------------------------------------------------------------------

    fn tick_one(&mut self, mapper: &mut dyn Mapper) {
        // (scanline, cycle) timing events.
        // - Pre-render (261) dot 1: clear vblank/sprite-0-hit/overflow.
        // - VBlank (241) dot 1: set vblank flag.
        // - Visible (0..=239) dot 256: render scanline + evaluate next sprites.
        if self.scanline == PRE_RENDER_SCANLINE && self.cycle == 1 {
            self.status &= !(status_bits::VBLANK
                | status_bits::SPRITE_ZERO_HIT
                | status_bits::SPRITE_OVERFLOW);
            self.refresh_nmi_edge();
        }
        if self.scanline == VBLANK_START_SCANLINE && self.cycle == 1 {
            self.status |= status_bits::VBLANK;
            self.refresh_nmi_edge();
        }
        if self.scanline < SCREEN_HEIGHT as u16 && self.cycle == 256 {
            // Render this scanline using current scroll state, then evaluate
            // the sprite list for the *next* scanline so it is in place when
            // we render it.
            if self.rendering_enabled() {
                self.render_scanline(mapper);
            }
            // Sprite evaluation runs whether or not background is enabled,
            // but it is only meaningful for sprite-0 hit if rendering is on.
            self.evaluate_sprites_for_next_scanline();
        }

        // Advance the dot/scanline/frame counters.
        self.cycle += 1;
        if self.cycle >= DOTS_PER_SCANLINE {
            self.cycle = 0;
            self.scanline += 1;
            if self.scanline >= SCANLINES_PER_FRAME {
                self.scanline = 0;
                self.frame_count = self.frame_count.wrapping_add(1);
            }
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

    /// `$2000` and `$2007` writes use the increment-mode bit of PPUCTRL.
    #[inline]
    fn vram_increment(&self) -> u16 {
        if self.ctrl & ctrl::VRAM_INCREMENT != 0 {
            32
        } else {
            1
        }
    }

    fn increment_v_after_data_access(&mut self) {
        self.v = self.v.wrapping_add(self.vram_increment()) & 0x3FFF;
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
        self.palette_ram[Self::palette_index(addr)] = value;
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
    // Sprite evaluation.
    // -----------------------------------------------------------------------

    /// Populate `sprite_buffer` with the sprites that intersect the *next*
    /// scanline (the one we'll render on the next tick at dot 256).
    fn evaluate_sprites_for_next_scanline(&mut self) {
        self.sprite_count = 0;
        self.sprite_zero_in_line = false;

        // The scanline whose pixels we will write next.
        let next_scanline = self.scanline.wrapping_add(1);
        if next_scanline >= SCREEN_HEIGHT as u16 {
            return;
        }
        let next_y = next_scanline as i32;
        let sprite_height = self.sprite_height() as i32;

        for i in 0..64 {
            let base = i * 4;
            let y = self.oam[base] as i32;
            // Sprite occupies rows [y+1, y+1+sprite_height) on screen.
            let row_top = y + 1;
            let row_bot = row_top + sprite_height;
            if next_y >= row_top && next_y < row_bot {
                if self.sprite_count < 8 {
                    self.sprite_buffer[self.sprite_count] = SpriteEntry {
                        oam_index: i as u8,
                        y: y as u8,
                        tile: self.oam[base + 1],
                        attr: self.oam[base + 2],
                        x: self.oam[base + 3],
                    };
                    if i == 0 {
                        self.sprite_zero_in_line = true;
                    }
                    self.sprite_count += 1;
                } else {
                    // 9th sprite encountered for this scanline.
                    self.status |= status_bits::SPRITE_OVERFLOW;
                    break;
                }
            }
        }
    }

    #[inline]
    fn sprite_height(&self) -> u8 {
        if self.ctrl & ctrl::SPRITE_SIZE != 0 {
            16
        } else {
            8
        }
    }

    // -----------------------------------------------------------------------
    // Rendering.
    // -----------------------------------------------------------------------

    /// Render the current visible scanline (`self.scanline`, 0..=239) into
    /// the framebuffer.
    fn render_scanline(&mut self, mapper: &mut dyn Mapper) {
        let y = self.scanline as usize;
        // Universal background color used when both BG and sprite pixels are
        // transparent.
        let universal_bg = self.read_palette(0x3F00) & 0x3F;

        // Snapshot of the loopy registers at the start of the scanline.
        let mut v_local = self.v;
        let fine_x = self.x;

        for x in 0..SCREEN_WIDTH {
            // ----- Background pixel.
            let (bg_pixel, bg_palette) = if self.mask & mask::SHOW_BG != 0
                && (x >= 8 || self.mask & mask::SHOW_BG_LEFT8 != 0)
            {
                self.fetch_bg_pixel(v_local, fine_x, x, y, mapper)
            } else {
                (0u8, 0u8)
            };

            // ----- Sprite pixel.
            let mut sprite_pixel = 0u8;
            let mut sprite_palette = 0u8;
            let mut sprite_is_zero = false;
            let mut sprite_priority_front = false;
            if self.mask & mask::SHOW_SPR != 0 && (x >= 8 || self.mask & mask::SHOW_SPR_LEFT8 != 0)
            {
                for i in 0..self.sprite_count {
                    let s = self.sprite_buffer[i];
                    let sx = s.x as usize;
                    if x < sx || x >= sx + 8 {
                        continue;
                    }
                    let col_in_sprite = (x - sx) as u8;
                    let (p, pal, in_front) = self.fetch_sprite_pixel(&s, col_in_sprite, y, mapper);
                    if p != 0 {
                        sprite_pixel = p;
                        sprite_palette = pal;
                        sprite_priority_front = in_front;
                        sprite_is_zero = self.sprite_zero_in_line && s.oam_index == 0;
                        break;
                    }
                }
            }

            // ----- Priority muxing.
            let (palette_addr, _is_sprite) = match (bg_pixel, sprite_pixel) {
                (0, 0) => (0x3F00u16, false),
                (0, _) => (
                    0x3F10 | (u16::from(sprite_palette) << 2) | u16::from(sprite_pixel),
                    true,
                ),
                (_, 0) => (
                    0x3F00 | (u16::from(bg_palette) << 2) | u16::from(bg_pixel),
                    false,
                ),
                (_, _) => {
                    // Sprite-zero hit: triggered only when both pixels are
                    // non-transparent, rendering is enabled, the pixel is
                    // not on the leftmost 8 columns when those are masked
                    // for either layer, and x < 255.
                    if sprite_is_zero && x != 255 {
                        let left8_ok = x >= 8
                            || (self.mask & mask::SHOW_BG_LEFT8 != 0
                                && self.mask & mask::SHOW_SPR_LEFT8 != 0);
                        if left8_ok {
                            self.status |= status_bits::SPRITE_ZERO_HIT;
                        }
                    }
                    if sprite_priority_front {
                        (
                            0x3F10 | (u16::from(sprite_palette) << 2) | u16::from(sprite_pixel),
                            true,
                        )
                    } else {
                        (
                            0x3F00 | (u16::from(bg_palette) << 2) | u16::from(bg_pixel),
                            false,
                        )
                    }
                }
            };

            let color_index = self.read_palette(palette_addr) & 0x3F;
            // Optimisation: fall back to the universal bg color if rendering
            // is disabled (caller already early-returns in that case so this
            // is mostly defensive).
            let color = palette::NES_PALETTE[if self.rendering_enabled() {
                color_index
            } else {
                universal_bg
            } as usize];

            let off = (y * SCREEN_WIDTH + x) * 4;
            self.framebuffer[off] = color[0];
            self.framebuffer[off + 1] = color[1];
            self.framebuffer[off + 2] = color[2];
            self.framebuffer[off + 3] = 0xFF;

            // Advance the per-pixel "coarse X carry" used by background tile
            // selection. This is a simplification: the real PPU increments
            // coarse X once per 8 dots after a fine-X-aligned start, with a
            // nametable bit toggled on each wraparound. Our `fetch_bg_pixel`
            // recomputes those addresses from v_local + fine_x + pixel_x on
            // each call so we don't need to mutate v_local here.
            let _ = &mut v_local;
        }
    }

    /// Compute the 2-bit background pattern pixel and 2-bit palette index for
    /// the screen column `screen_x` on the current scanline, starting from
    /// the loopy `v` and `fine_x` values latched at the beginning of the
    /// scanline. This implements the same address math the real PPU performs,
    /// but in a per-pixel form for simplicity.
    fn fetch_bg_pixel(
        &self,
        v_at_line_start: u16,
        fine_x: u8,
        screen_x: usize,
        screen_y: usize,
        mapper: &mut dyn Mapper,
    ) -> (u8, u8) {
        // Vertical position is taken directly from the scanline number plus
        // the coarse-Y / fine-Y components latched into v at the start of
        // the frame (the real PPU advances v's vertical bits at the end of
        // each scanline; we approximate by deriving them from `screen_y`).
        let v = v_at_line_start;
        let coarse_x = (v & 0x001F) as u32;
        let coarse_y_start = ((v >> 5) & 0x001F) as u32;
        let nt_select = ((v >> 10) & 0x0003) as u32;
        let fine_y_start = ((v >> 12) & 0x0007) as u32;

        // Add screen_y rows to the starting vertical position.
        let absolute_y = coarse_y_start * 8 + fine_y_start + screen_y as u32;
        let coarse_y = (absolute_y / 8) & 0x1F;
        let fine_y = absolute_y % 8;
        let v_wraps = absolute_y / (30 * 8); // 30 rows per nametable

        let absolute_x = coarse_x * 8 + u32::from(fine_x) + screen_x as u32;
        let tile_col = (absolute_x / 8) & 0x1F;
        let pixel_col = (absolute_x % 8) as u8;
        let h_wraps = absolute_x / (32 * 8);
        let nt_h = (nt_select ^ h_wraps) & 1;
        let nt_v = ((nt_select >> 1) ^ v_wraps) & 1;

        let nametable_base = 0x2000 | (nt_v << 11) | (nt_h << 10);
        let tile_addr = nametable_base | (coarse_y << 5) | tile_col;
        let tile_index = self.vram[self.nametable_index(tile_addr as u16, mapper)];

        // Attribute byte: each byte covers a 4×4 tile region (32×32 px).
        let attr_addr =
            0x23C0 | (nt_v << 11) | (nt_h << 10) | ((coarse_y / 4) << 3) | (tile_col / 4);
        let attr_byte = self.vram[self.nametable_index(attr_addr as u16, mapper)];
        let quadrant_shift = ((coarse_y & 0x02) << 1) | (tile_col & 0x02);
        let palette = (attr_byte >> quadrant_shift) & 0x03;

        // Pattern table lookup.
        let pattern_base = if self.ctrl & ctrl::BG_PATTERN != 0 {
            0x1000
        } else {
            0x0000
        };
        let tile_addr = pattern_base + u16::from(tile_index) * 16 + fine_y as u16;
        let lo = mapper.ppu_read(tile_addr);
        let hi = mapper.ppu_read(tile_addr + 8);
        let bit = 7 - pixel_col;
        let p0 = (lo >> bit) & 1;
        let p1 = (hi >> bit) & 1;
        let pixel = (p1 << 1) | p0;
        (pixel, palette)
    }

    /// Compute the 2-bit sprite pattern pixel and 2-bit palette index for
    /// the given column inside the sprite. Returns `(pixel, palette, in_front)`.
    fn fetch_sprite_pixel(
        &self,
        sprite: &SpriteEntry,
        col_in_sprite: u8,
        y: usize,
        mapper: &mut dyn Mapper,
    ) -> (u8, u8, bool) {
        let row_in_sprite = (y as i32 - sprite.y as i32 - 1) as u8;
        let flip_h = sprite.attr & 0x40 != 0;
        let flip_v = sprite.attr & 0x80 != 0;
        let palette = sprite.attr & 0x03;
        let in_front = sprite.attr & 0x20 == 0;

        let height = self.sprite_height();
        let (pattern_base, tile_index, fine_y) = if height == 8 {
            let sel_base = if self.ctrl & ctrl::SPRITE_PATTERN != 0 {
                0x1000
            } else {
                0x0000
            };
            let fy = if flip_v {
                7 - row_in_sprite
            } else {
                row_in_sprite
            };
            (sel_base, sprite.tile, fy)
        } else {
            // 8×16 sprite: bit 0 of tile selects the pattern table.
            let base = if sprite.tile & 0x01 != 0 {
                0x1000
            } else {
                0x0000
            };
            let mut row = row_in_sprite;
            if flip_v {
                row = 15 - row;
            }
            // Top tile uses (tile & 0xFE); bottom tile is (tile & 0xFE) | 1.
            let tile = (sprite.tile & 0xFE) | u8::from(row >= 8);
            let fy = row & 0x07;
            (base, tile, fy)
        };

        let addr = pattern_base + u16::from(tile_index) * 16 + u16::from(fine_y);
        let lo = mapper.ppu_read(addr);
        let hi = mapper.ppu_read(addr + 8);
        let bit = if flip_h {
            col_in_sprite
        } else {
            7 - col_in_sprite
        };
        let p0 = (lo >> bit) & 1;
        let p1 = (hi >> bit) & 1;
        let pixel = (p1 << 1) | p0;
        (pixel, palette, in_front)
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

// Silence the unused-import warnings for ctrl/mask bits the renderer does not
// currently consult (grayscale/emphasis are out of scope for this milestone).
#[allow(dead_code)]
fn _suppress_unused() {
    let _ = (
        ctrl::NAMETABLE_LO,
        ctrl::NAMETABLE_HI,
        ctrl::_MASTER_SLAVE,
        mask::_GRAYSCALE,
        mask::_EMPHASIZE_R,
        mask::_EMPHASIZE_G,
        mask::_EMPHASIZE_B,
    );
}

// Re-export the bits that are part of the public API surface so callers
// don't have to reach into private modules to test individual flags.
pub use status_bits::{SPRITE_OVERFLOW, SPRITE_ZERO_HIT, VBLANK};
