//! NES 2A03 audio processing unit (APU).
//!
//! This module implements the five-channel APU producing mono `f32` samples
//! at a host-provided sample rate. Scope of the implementation:
//!
//! - **Pulse 1 / Pulse 2**: full envelope, sweep (with the canonical
//!   pulse-1 ones-complement / pulse-2 twos-complement negation difference),
//!   length counter, duty cycle.
//! - **Triangle**: linear counter, length counter, 32-step sequence.
//! - **Noise**: 15-bit LFSR with mode-0 (bit 1) and mode-1 (bit 6) feedback,
//!   envelope, length counter.
//! - **DMC**: delta-modulation sample playback with the one-byte sample
//!   buffer, output shift register, loop / IRQ handling and CPU-memory DMA
//!   fetches (see [`Apu::step_with_memory`]).
//! - **Frame counter**: 4-step and 5-step modes; frame IRQ on the
//!   4-step mode when not inhibited.
//!
//! ## Public surface
//!
//! - [`Apu::new`] / [`Apu::reset`] / [`Apu::with_sample_rate`]
//! - [`Apu::step`] — advance the APU by `cpu_cycles` 6502 clocks. Drives
//!   the per-channel timers, the frame counter, and accumulates mixer
//!   samples at the host sample rate.
//! - [`Apu::drain_samples`] — move accumulated samples into the caller's
//!   `Vec<f32>`.
//! - [`Apu::read_status`] — handles `$4015` reads; clears the frame
//!   interrupt flag as a side effect (matching the real hardware).
//! - [`Apu::write_register`] — handles writes to `$4000-$4017`.
//! - [`Apu::irq_pending`] — true when either the frame counter or the DMC
//!   has an outstanding IRQ.
//!
//! ## Timing model
//!
//! The APU is clocked from the CPU. Pulse channels advance their timers on
//! every *APU* cycle (one APU cycle = two CPU cycles); the triangle, noise
//! and DMC timers advance on every CPU cycle (their period tables are in
//! CPU cycles). DMC sample bytes are fetched through
//! [`Apu::step_with_memory`], which the NES bus wires to cartridge space. Frame-counter events are
//! placed at the canonical NTSC offsets:
//!
//! | Step | 4-step (CPU cycles) | 5-step (CPU cycles) | Quarter | Half  |
//! |------|---------------------|---------------------|---------|-------|
//! | 1    | 7457                | 7457                | ✓       |       |
//! | 2    | 14913               | 14913               | ✓       | ✓     |
//! | 3    | 22371               | 22371               | ✓       |       |
//! | 4    | 29829 (+ IRQ)       | 29829               | ✓       | ✓ (4) |
//! | 5    | wrap @ 29830        | 37281               | ✓       | ✓ (5) |
//! | wrap |                     | 37282               |         |       |
//!
//! Sample generation uses an integer fractional accumulator so that for
//! `N` CPU cycles `drain_samples` yields `floor((N + carry) * sample_rate /
//! cpu_hz)` samples, with the unconsumed remainder carried into the next
//! `step` call (the `±1` accuracy promised by the unit tests).

#[cfg(test)]
mod tests;

/// NTSC NES CPU clock in hertz.
pub const NES_CPU_HZ: u32 = 1_789_773;

/// Default audio sample rate used when one isn't supplied to
/// [`Apu::with_sample_rate`].
pub const DEFAULT_SAMPLE_RATE: u32 = 44_100;

/// Cut-off of the output DC-blocking high-pass filter (the NES's first
/// analogue output stage sits at ~90 Hz).
const HIGH_PASS_HZ: f32 = 90.0;

/// Coefficient of a one-pole RC high-pass at `cutoff_hz`.
fn high_pass_alpha(sample_rate: u32, cutoff_hz: f32) -> f32 {
    let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
    let dt = 1.0 / sample_rate as f32;
    rc / (rc + dt)
}

/// Length-counter load table. Indexed by the upper 5 bits of writes to
/// `$4003`/`$4007`/`$400B`/`$400F`.
pub(crate) const LENGTH_TABLE: [u8; 32] = [
    10, 254, 20, 2, 40, 4, 80, 6, 160, 8, 60, 10, 14, 12, 26, 14, 12, 16, 24, 18, 48, 20, 96, 22,
    192, 24, 72, 26, 16, 28, 32, 30,
];

/// Noise channel timer reload values (NTSC).
pub(crate) const NOISE_PERIODS: [u16; 16] = [
    4, 8, 16, 32, 64, 96, 128, 160, 202, 254, 380, 508, 762, 1016, 2034, 4068,
];

/// DMC sample rate timer reload values (NTSC).
pub(crate) const DMC_RATES: [u16; 16] = [
    428, 380, 340, 320, 286, 254, 226, 214, 190, 160, 142, 128, 106, 84, 72, 54,
];

/// Pulse channel duty-cycle waveforms (8 steps each).
pub(crate) const PULSE_DUTY: [[u8; 8]; 4] = [
    [0, 1, 0, 0, 0, 0, 0, 0], // 12.5%
    [0, 1, 1, 0, 0, 0, 0, 0], // 25%
    [0, 1, 1, 1, 1, 0, 0, 0], // 50%
    [1, 0, 0, 1, 1, 1, 1, 1], // 25% negated
];

/// Triangle channel 32-step sequence.
pub(crate) const TRIANGLE_SEQUENCE: [u8; 32] = [
    15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12,
    13, 14, 15,
];

// -----------------------------------------------------------------------
// Length counter / envelope / sweep building blocks.
// -----------------------------------------------------------------------

/// Length counter (shared by pulse, triangle, and noise).
#[derive(Debug, Default)]
pub(crate) struct LengthCounter {
    enabled: bool,
    /// "Halt" for pulse/noise; "control" for triangle. Either way it
    /// freezes the count.
    halt: bool,
    counter: u8,
}

impl LengthCounter {
    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        if !enabled {
            self.counter = 0;
        }
    }

    fn load(&mut self, idx: u8) {
        if self.enabled {
            self.counter = LENGTH_TABLE[(idx & 0x1F) as usize];
        }
    }

    fn clock(&mut self) {
        if !self.halt && self.counter > 0 {
            self.counter -= 1;
        }
    }

    pub(crate) fn active(&self) -> bool {
        self.counter > 0
    }

    /// Current count. Used by tests and diagnostics.
    #[cfg(test)]
    pub(crate) fn value(&self) -> u8 {
        self.counter
    }
}

/// Envelope generator (shared by pulse and noise).
#[derive(Debug, Default)]
pub(crate) struct Envelope {
    start: bool,
    loop_flag: bool,
    constant: bool,
    /// Divider reload value and, in constant-volume mode, the output level.
    volume: u8,
    decay: u8,
    divider: u8,
}

impl Envelope {
    fn clock(&mut self) {
        if self.start {
            self.start = false;
            self.decay = 15;
            self.divider = self.volume;
        } else if self.divider == 0 {
            self.divider = self.volume;
            if self.decay > 0 {
                self.decay -= 1;
            } else if self.loop_flag {
                self.decay = 15;
            }
        } else {
            self.divider -= 1;
        }
    }

    fn output(&self) -> u8 {
        if self.constant {
            self.volume
        } else {
            self.decay
        }
    }
}

/// Pulse-channel sweep unit.
#[derive(Debug, Default)]
struct Sweep {
    enabled: bool,
    period: u8,
    negate: bool,
    shift: u8,
    reload: bool,
    divider: u8,
    /// Cached "muted by sweep" state, recomputed on every sweep clock and
    /// on every period update.
    mute: bool,
}

impl Sweep {
    fn write(&mut self, v: u8) {
        self.enabled = (v & 0x80) != 0;
        self.period = (v >> 4) & 0x07;
        self.negate = (v & 0x08) != 0;
        self.shift = v & 0x07;
        self.reload = true;
    }
}

// -----------------------------------------------------------------------
// Pulse channel.
// -----------------------------------------------------------------------

/// One of the two pulse channels.
pub(crate) struct Pulse {
    /// `0` = pulse 1 (ones-complement negate), `1` = pulse 2
    /// (twos-complement negate).
    channel: u8,
    enabled: bool,
    duty: u8,
    duty_step: u8,
    timer_period: u16,
    timer: u16,
    envelope: Envelope,
    sweep: Sweep,
    pub(crate) length: LengthCounter,
}

impl Pulse {
    fn new(channel: u8) -> Self {
        Self {
            channel,
            enabled: false,
            duty: 0,
            duty_step: 0,
            timer_period: 0,
            timer: 0,
            envelope: Envelope::default(),
            sweep: Sweep::default(),
            length: LengthCounter::default(),
        }
    }

    fn write_ctrl(&mut self, v: u8) {
        self.duty = (v >> 6) & 0x03;
        let halt = (v & 0x20) != 0;
        self.length.halt = halt;
        self.envelope.loop_flag = halt;
        self.envelope.constant = (v & 0x10) != 0;
        self.envelope.volume = v & 0x0F;
    }

    fn write_sweep(&mut self, v: u8) {
        self.sweep.write(v);
        self.update_sweep_mute();
    }

    fn write_lo(&mut self, v: u8) {
        self.timer_period = (self.timer_period & 0xFF00) | u16::from(v);
        self.update_sweep_mute();
    }

    fn write_hi(&mut self, v: u8) {
        self.timer_period = (self.timer_period & 0x00FF) | ((u16::from(v) & 0x07) << 8);
        self.length.load(v >> 3);
        self.duty_step = 0;
        self.envelope.start = true;
        self.update_sweep_mute();
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.length.set_enabled(enabled);
    }

    fn target_period(&self) -> u16 {
        let change = self.timer_period >> self.sweep.shift;
        if self.sweep.negate {
            // Pulse 1 negates with ones-complement (-c - 1); pulse 2 with
            // twos-complement (-c).
            let bias = if self.channel == 0 { 1 } else { 0 };
            self.timer_period.saturating_sub(change + bias)
        } else {
            self.timer_period.saturating_add(change)
        }
    }

    fn update_sweep_mute(&mut self) {
        self.sweep.mute = self.timer_period < 8 || self.target_period() > 0x7FF;
    }

    fn clock_sweep(&mut self) {
        // Recompute mute before deciding whether to apply.
        self.update_sweep_mute();
        if self.sweep.divider == 0 && self.sweep.enabled && !self.sweep.mute && self.sweep.shift > 0
        {
            self.timer_period = self.target_period();
            self.update_sweep_mute();
        }
        if self.sweep.divider == 0 || self.sweep.reload {
            self.sweep.divider = self.sweep.period;
            self.sweep.reload = false;
        } else {
            self.sweep.divider -= 1;
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.timer_period;
            self.duty_step = (self.duty_step + 1) & 7;
        } else {
            self.timer -= 1;
        }
    }

    fn output(&self) -> u8 {
        if !self.enabled
            || !self.length.active()
            || self.sweep.mute
            || self.timer_period < 8
            || PULSE_DUTY[self.duty as usize][self.duty_step as usize] == 0
        {
            0
        } else {
            self.envelope.output()
        }
    }
}

// -----------------------------------------------------------------------
// Triangle channel.
// -----------------------------------------------------------------------

pub(crate) struct Triangle {
    enabled: bool,
    timer_period: u16,
    timer: u16,
    sequence_step: u8,
    pub(crate) length: LengthCounter,
    pub(crate) linear_counter: u8,
    linear_reload_value: u8,
    linear_reload_flag: bool,
    /// Triangle "control" flag — also acts as the length-counter halt.
    control_flag: bool,
}

impl Triangle {
    fn new() -> Self {
        Self {
            enabled: false,
            timer_period: 0,
            timer: 0,
            sequence_step: 0,
            length: LengthCounter::default(),
            linear_counter: 0,
            linear_reload_value: 0,
            linear_reload_flag: false,
            control_flag: false,
        }
    }

    fn write_ctrl(&mut self, v: u8) {
        let control = (v & 0x80) != 0;
        self.control_flag = control;
        self.length.halt = control;
        self.linear_reload_value = v & 0x7F;
    }

    fn write_lo(&mut self, v: u8) {
        self.timer_period = (self.timer_period & 0xFF00) | u16::from(v);
    }

    fn write_hi(&mut self, v: u8) {
        self.timer_period = (self.timer_period & 0x00FF) | ((u16::from(v) & 0x07) << 8);
        self.length.load(v >> 3);
        self.linear_reload_flag = true;
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.length.set_enabled(enabled);
    }

    pub(crate) fn clock_linear(&mut self) {
        if self.linear_reload_flag {
            self.linear_counter = self.linear_reload_value;
        } else if self.linear_counter > 0 {
            self.linear_counter -= 1;
        }
        if !self.control_flag {
            self.linear_reload_flag = false;
        }
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.timer_period;
            if self.length.active() && self.linear_counter > 0 {
                self.sequence_step = (self.sequence_step + 1) & 0x1F;
            }
        } else {
            self.timer -= 1;
        }
    }

    fn output(&self) -> u8 {
        if !self.enabled || self.timer_period < 2 {
            // Very-high-frequency triangle is silenced to avoid the
            // audible click that running the sequencer would produce.
            0
        } else {
            TRIANGLE_SEQUENCE[self.sequence_step as usize]
        }
    }
}

// -----------------------------------------------------------------------
// Noise channel.
// -----------------------------------------------------------------------

pub(crate) struct Noise {
    enabled: bool,
    mode: bool,
    timer_period: u16,
    timer: u16,
    /// 15-bit linear-feedback shift register, initialised to `1` at power-on.
    pub(crate) lfsr: u16,
    envelope: Envelope,
    pub(crate) length: LengthCounter,
}

impl Noise {
    fn new() -> Self {
        Self {
            enabled: false,
            mode: false,
            timer_period: NOISE_PERIODS[0],
            timer: 0,
            lfsr: 1,
            envelope: Envelope::default(),
            length: LengthCounter::default(),
        }
    }

    fn write_ctrl(&mut self, v: u8) {
        let halt = (v & 0x20) != 0;
        self.length.halt = halt;
        self.envelope.loop_flag = halt;
        self.envelope.constant = (v & 0x10) != 0;
        self.envelope.volume = v & 0x0F;
    }

    fn write_period(&mut self, v: u8) {
        self.mode = (v & 0x80) != 0;
        self.timer_period = NOISE_PERIODS[(v & 0x0F) as usize];
    }

    fn write_length(&mut self, v: u8) {
        self.length.load(v >> 3);
        self.envelope.start = true;
    }

    fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
        self.length.set_enabled(enabled);
    }

    /// Single LFSR shift. Exposed at crate-level for the deterministic
    /// 16-step golden-trace test.
    pub(crate) fn shift_lfsr(&mut self) {
        let bit0 = self.lfsr & 1;
        let bit_other = if self.mode {
            (self.lfsr >> 6) & 1
        } else {
            (self.lfsr >> 1) & 1
        };
        let feedback = bit0 ^ bit_other;
        self.lfsr = (self.lfsr >> 1) | (feedback << 14);
    }

    fn clock_timer(&mut self) {
        if self.timer == 0 {
            self.timer = self.timer_period;
            self.shift_lfsr();
        } else {
            self.timer -= 1;
        }
    }

    fn output(&self) -> u8 {
        if !self.enabled || !self.length.active() || (self.lfsr & 1) != 0 {
            0
        } else {
            self.envelope.output()
        }
    }
}

// -----------------------------------------------------------------------
// DMC channel.
// -----------------------------------------------------------------------

pub(crate) struct Dmc {
    irq_enabled: bool,
    loop_flag: bool,
    /// Output-unit period in CPU cycles.
    rate: u16,
    timer: u16,
    output_level: u8,
    sample_address: u16,
    sample_length: u16,
    current_address: u16,
    pub(crate) bytes_remaining: u16,
    pub(crate) irq_flag: bool,
    /// One-byte sample buffer filled by the DMA reader.
    sample_buffer: Option<u8>,
    shift_register: u8,
    bits_remaining: u8,
    silence: bool,
}

impl Dmc {
    fn new() -> Self {
        Self {
            irq_enabled: false,
            loop_flag: false,
            rate: DMC_RATES[0],
            timer: DMC_RATES[0] - 1,
            output_level: 0,
            sample_address: 0xC000,
            sample_length: 1,
            current_address: 0xC000,
            bytes_remaining: 0,
            irq_flag: false,
            sample_buffer: None,
            shift_register: 0,
            bits_remaining: 8,
            silence: true,
        }
    }

    fn write_ctrl(&mut self, v: u8) {
        self.irq_enabled = (v & 0x80) != 0;
        self.loop_flag = (v & 0x40) != 0;
        self.rate = DMC_RATES[(v & 0x0F) as usize];
        if !self.irq_enabled {
            self.irq_flag = false;
        }
    }

    fn write_da(&mut self, v: u8) {
        self.output_level = v & 0x7F;
    }

    fn write_addr(&mut self, v: u8) {
        self.sample_address = 0xC000 | (u16::from(v) << 6);
    }

    fn write_length(&mut self, v: u8) {
        self.sample_length = (u16::from(v) << 4) | 1;
    }

    fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.bytes_remaining = 0;
        } else if self.bytes_remaining == 0 {
            self.restart();
        }
        // A write to $4015 always clears the DMC IRQ flag.
        self.irq_flag = false;
    }

    fn restart(&mut self) {
        self.current_address = self.sample_address;
        self.bytes_remaining = self.sample_length;
    }

    /// Address the memory reader must fetch next, if the sample buffer is
    /// empty and bytes remain in the current sample.
    fn dma_request(&self) -> Option<u16> {
        if self.sample_buffer.is_none() && self.bytes_remaining > 0 {
            Some(self.current_address)
        } else {
            None
        }
    }

    /// Deliver the byte fetched for [`Dmc::dma_request`].
    fn dma_complete(&mut self, byte: u8) {
        self.sample_buffer = Some(byte);
        self.current_address = if self.current_address == 0xFFFF {
            0x8000
        } else {
            self.current_address + 1
        };
        self.bytes_remaining = self.bytes_remaining.saturating_sub(1);
        if self.bytes_remaining == 0 {
            if self.loop_flag {
                self.restart();
            } else if self.irq_enabled {
                self.irq_flag = true;
            }
        }
    }

    /// Clocked on every CPU cycle; the rate table is expressed in CPU cycles.
    fn clock_timer(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
            return;
        }
        self.timer = self.rate.saturating_sub(1);
        if !self.silence {
            if self.shift_register & 1 != 0 {
                if self.output_level <= 125 {
                    self.output_level += 2;
                }
            } else if self.output_level >= 2 {
                self.output_level -= 2;
            }
        }
        self.shift_register >>= 1;
        self.bits_remaining = self.bits_remaining.saturating_sub(1);
        if self.bits_remaining == 0 {
            self.bits_remaining = 8;
            match self.sample_buffer.take() {
                Some(b) => {
                    self.shift_register = b;
                    self.silence = false;
                }
                None => self.silence = true,
            }
        }
    }

    fn output(&self) -> u8 {
        self.output_level
    }
}

// -----------------------------------------------------------------------
// Frame counter constants.
// -----------------------------------------------------------------------

// CPU-cycle offsets of frame-counter events (NTSC).
const FRAME_4STEP: [u32; 4] = [7457, 14913, 22371, 29829];
const FRAME_5STEP: [u32; 5] = [7457, 14913, 22371, 29829, 37281];
const FRAME_4STEP_WRAP: u32 = 29830;
const FRAME_5STEP_WRAP: u32 = 37282;

// -----------------------------------------------------------------------
// APU itself.
// -----------------------------------------------------------------------

/// The 2A03 audio processing unit. See the module-level docs for scope.
pub struct Apu {
    pub(crate) pulse1: Pulse,
    pub(crate) pulse2: Pulse,
    pub(crate) triangle: Triangle,
    pub(crate) noise: Noise,
    pub(crate) dmc: Dmc,

    // Frame counter.
    frame_mode: u8, // 0 = 4-step, 1 = 5-step
    frame_irq_inhibit: bool,
    pub(crate) frame_irq: bool,
    frame_cycle: u32,
    /// Mode written to `$4017`, applied once `frame_reset_delay` expires.
    pending_frame_mode: Option<u8>,
    /// Last value written to `$4017`; re-applied on a warm reset.
    last_frame_counter_write: u8,
    frame_reset_delay: u8,

    // Triangle clocks on every CPU cycle; pulse/noise/DMC on every other
    // CPU cycle (the "APU cycle"). This toggles to select APU cycles.
    apu_phase: bool,

    // Sample generation.
    sample_rate: u32,
    sample_accumulator: u64,
    samples: Vec<f32>,
    /// Box-filter accumulator over the CPU cycles of the current sample.
    mix_sum: f32,
    mix_count: u32,
    /// One-pole DC-blocking high-pass filter state.
    hp_alpha: f32,
    hp_prev_in: f32,
    hp_prev_out: f32,
}

impl Default for Apu {
    fn default() -> Self {
        Self::new()
    }
}

impl Apu {
    /// Build a power-on APU running at [`DEFAULT_SAMPLE_RATE`].
    pub fn new() -> Self {
        Self::with_sample_rate(DEFAULT_SAMPLE_RATE)
    }

    /// Build a power-on APU running at the supplied sample rate (Hz).
    pub fn with_sample_rate(sample_rate: u32) -> Self {
        assert!(sample_rate > 0, "sample_rate must be positive");
        Self {
            pulse1: Pulse::new(0),
            pulse2: Pulse::new(1),
            triangle: Triangle::new(),
            noise: Noise::new(),
            dmc: Dmc::new(),
            frame_mode: 0,
            frame_irq_inhibit: false,
            frame_irq: false,
            frame_cycle: 0,
            pending_frame_mode: None,
            last_frame_counter_write: 0,
            frame_reset_delay: 0,
            apu_phase: false,
            sample_rate,
            sample_accumulator: 0,
            samples: Vec::new(),
            mix_sum: 0.0,
            mix_count: 0,
            hp_alpha: high_pass_alpha(sample_rate, HIGH_PASS_HZ),
            hp_prev_in: 0.0,
            hp_prev_out: 0.0,
        }
    }

    /// Reset (RESET pin asserted) — clears registers and silences output.
    /// Sample rate and buffered samples are preserved.
    pub fn reset(&mut self) {
        let sample_rate = self.sample_rate;
        let mut next = Self::with_sample_rate(sample_rate);
        next.samples = std::mem::take(&mut self.samples);
        *self = next;
    }

    /// Warm reset (the console's RESET button): unlike [`Apu::reset`],
    /// channel registers survive. `$4015` is cleared (silencing every
    /// channel and its length counter) and the last `$4017` value is written
    /// again, restarting the frame sequencer.
    pub fn soft_reset(&mut self) {
        self.write_register(0x4015, 0x00);
        self.frame_irq = false;
        let last = self.last_frame_counter_write;
        self.write_register(0x4017, last);
        // The reset sequence itself provides the write delay: the caller
        // ticks the APU through the CPU's reset cycles.
        self.apply_frame_counter_mode();
    }

    /// The configured host sample rate (Hz).
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Whether any APU interrupt line is asserted (frame IRQ or DMC IRQ).
    pub fn irq_pending(&self) -> bool {
        self.frame_irq || self.dmc.irq_flag
    }

    /// Service a CPU read from `$4015`. Reading the status register
    /// clears the frame-interrupt flag as a side effect.
    pub fn read_status(&mut self) -> u8 {
        let mut status = 0u8;
        if self.pulse1.length.active() {
            status |= 0x01;
        }
        if self.pulse2.length.active() {
            status |= 0x02;
        }
        if self.triangle.length.active() {
            status |= 0x04;
        }
        if self.noise.length.active() {
            status |= 0x08;
        }
        if self.dmc.bytes_remaining > 0 {
            status |= 0x10;
        }
        if self.frame_irq {
            status |= 0x40;
        }
        if self.dmc.irq_flag {
            status |= 0x80;
        }
        // Reading $4015 clears the frame interrupt flag (but not DMC).
        self.frame_irq = false;
        status
    }

    /// Service a CPU write to `$4000-$4017`. Addresses outside the
    /// documented register range are ignored.
    pub fn write_register(&mut self, addr: u16, value: u8) {
        match addr {
            0x4000 => self.pulse1.write_ctrl(value),
            0x4001 => self.pulse1.write_sweep(value),
            0x4002 => self.pulse1.write_lo(value),
            0x4003 => self.pulse1.write_hi(value),
            0x4004 => self.pulse2.write_ctrl(value),
            0x4005 => self.pulse2.write_sweep(value),
            0x4006 => self.pulse2.write_lo(value),
            0x4007 => self.pulse2.write_hi(value),
            0x4008 => self.triangle.write_ctrl(value),
            0x4009 => {} // unused on 2A03
            0x400A => self.triangle.write_lo(value),
            0x400B => self.triangle.write_hi(value),
            0x400C => self.noise.write_ctrl(value),
            0x400D => {} // unused
            0x400E => self.noise.write_period(value),
            0x400F => self.noise.write_length(value),
            0x4010 => self.dmc.write_ctrl(value),
            0x4011 => self.dmc.write_da(value),
            0x4012 => self.dmc.write_addr(value),
            0x4013 => self.dmc.write_length(value),
            0x4015 => {
                self.pulse1.set_enabled((value & 0x01) != 0);
                self.pulse2.set_enabled((value & 0x02) != 0);
                self.triangle.set_enabled((value & 0x04) != 0);
                self.noise.set_enabled((value & 0x08) != 0);
                self.dmc.set_enabled((value & 0x10) != 0);
            }
            0x4017 => {
                self.last_frame_counter_write = value;
                self.frame_irq_inhibit = (value & 0x40) != 0;
                if self.frame_irq_inhibit {
                    self.frame_irq = false;
                }
                // The new mode takes effect (and the sequencer restarts) 3
                // CPU cycles after the write if it lands on an APU cycle,
                // 4 cycles otherwise.
                self.pending_frame_mode = Some((value >> 7) & 0x01);
                self.frame_reset_delay = if self.apu_phase { 4 } else { 3 };
            }
            _ => {}
        }
    }

    /// Advance the APU by `cpu_cycles` 6502 clocks.
    ///
    /// DMC sample fetches are answered with `0` because no memory is
    /// attached; the NES bus uses [`Apu::step_with_memory`] instead.
    pub fn step(&mut self, cpu_cycles: u32) {
        let _ = self.step_with_memory(cpu_cycles, |_| 0);
    }

    /// Advance the APU by `cpu_cycles` 6502 clocks, servicing DMC sample
    /// DMA through `read`. Returns the number of CPU stall cycles the DMA
    /// fetches cost (4 per fetched byte), which the caller must add to the
    /// running instruction's cycle count.
    pub fn step_with_memory<F: FnMut(u16) -> u8>(&mut self, cpu_cycles: u32, mut read: F) -> u32 {
        let mut stall = 0;
        for _ in 0..cpu_cycles {
            self.tick_one_cpu_cycle();
            if let Some(addr) = self.dmc.dma_request() {
                let byte = read(addr);
                self.dmc.dma_complete(byte);
                stall += 4;
            }
        }
        stall
    }

    /// Move accumulated mono samples into `out`, leaving `self`'s sample
    /// buffer empty.
    pub fn drain_samples(&mut self, out: &mut Vec<f32>) {
        out.append(&mut self.samples);
    }

    /// Number of samples currently buffered (test/diagnostic).
    pub fn buffered_samples(&self) -> usize {
        self.samples.len()
    }

    // ----- internal helpers ------------------------------------------------

    fn tick_one_cpu_cycle(&mut self) {
        // Triangle, noise and DMC timers count CPU cycles (their period
        // tables are expressed in CPU cycles).
        self.triangle.clock_timer();
        self.noise.clock_timer();
        self.dmc.clock_timer();

        // Pulse timers clock every other CPU cycle (APU cycle).
        if self.apu_phase {
            self.pulse1.clock_timer();
            self.pulse2.clock_timer();
        }
        self.apu_phase = !self.apu_phase;

        // Frame counter.
        self.frame_cycle += 1;
        self.advance_frame_counter();
        if self.pending_frame_mode.is_some() {
            self.frame_reset_delay = self.frame_reset_delay.saturating_sub(1);
            if self.frame_reset_delay == 0 {
                self.apply_frame_counter_mode();
            }
        }

        // Sample accumulator. Emit a sample whenever the fractional
        // accumulator crosses the CPU clock; this yields exactly
        // floor(cycles * sample_rate / cpu_hz) samples per `step` call.
        //
        // Every CPU cycle's mixer output is accumulated and averaged into
        // the emitted sample (a box filter) so ultrasonic content does not
        // alias into the audible band, then a one-pole high-pass removes the
        // mixer's DC offset (like the NES's own output coupling capacitor).
        self.mix_sum += self.mix_sample();
        self.mix_count += 1;
        self.sample_accumulator += u64::from(self.sample_rate);
        if self.sample_accumulator >= u64::from(NES_CPU_HZ) {
            self.sample_accumulator -= u64::from(NES_CPU_HZ);
            let avg = self.mix_sum / self.mix_count as f32;
            self.mix_sum = 0.0;
            self.mix_count = 0;
            let out = self.hp_alpha * (self.hp_prev_out + avg - self.hp_prev_in);
            self.hp_prev_in = avg;
            self.hp_prev_out = out;
            self.samples.push(out);
        }
    }

    /// Apply a pending `$4017` write: switch mode and restart the sequencer.
    fn apply_frame_counter_mode(&mut self) {
        let Some(mode) = self.pending_frame_mode.take() else {
            return;
        };
        self.frame_mode = mode;
        self.frame_cycle = 0;
        // Mode 1 (5-step) issues an immediate quarter + half clock.
        if mode == 1 {
            self.clock_quarter_frame();
            self.clock_half_frame();
        }
    }

    fn advance_frame_counter(&mut self) {
        match self.frame_mode {
            0 => {
                // 4-step mode.
                if self.frame_cycle == FRAME_4STEP[0] {
                    self.clock_quarter_frame();
                } else if self.frame_cycle == FRAME_4STEP[1] {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                } else if self.frame_cycle == FRAME_4STEP[2] {
                    self.clock_quarter_frame();
                } else if self.frame_cycle == FRAME_4STEP[3] - 1 {
                    // The frame IRQ flag is asserted on three consecutive
                    // cycles: 29828, 29829 and 29830.
                    if !self.frame_irq_inhibit {
                        self.frame_irq = true;
                    }
                } else if self.frame_cycle == FRAME_4STEP[3] {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                    if !self.frame_irq_inhibit {
                        self.frame_irq = true;
                    }
                } else if self.frame_cycle >= FRAME_4STEP_WRAP {
                    if !self.frame_irq_inhibit {
                        self.frame_irq = true;
                    }
                    self.frame_cycle = 0;
                }
            }
            _ => {
                // 5-step mode.
                if self.frame_cycle == FRAME_5STEP[0] {
                    self.clock_quarter_frame();
                } else if self.frame_cycle == FRAME_5STEP[1] {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                } else if self.frame_cycle == FRAME_5STEP[2] {
                    self.clock_quarter_frame();
                } else if self.frame_cycle == FRAME_5STEP[3] {
                    // No clocks on this step in 5-step mode.
                } else if self.frame_cycle == FRAME_5STEP[4] {
                    self.clock_quarter_frame();
                    self.clock_half_frame();
                } else if self.frame_cycle >= FRAME_5STEP_WRAP {
                    self.frame_cycle = 0;
                }
            }
        }
    }

    fn clock_quarter_frame(&mut self) {
        self.pulse1.envelope.clock();
        self.pulse2.envelope.clock();
        self.noise.envelope.clock();
        self.triangle.clock_linear();
    }

    fn clock_half_frame(&mut self) {
        self.pulse1.length.clock();
        self.pulse2.length.clock();
        self.triangle.length.clock();
        self.noise.length.clock();
        self.pulse1.clock_sweep();
        self.pulse2.clock_sweep();
    }

    fn mix_sample(&self) -> f32 {
        // Linear approximation of the NES mixer (close enough for FR-18).
        // Pulse mix: pulse_out = 0.00752 * (p1 + p2)
        // TND mix:   tnd_out   = 0.00851 * t + 0.00494 * n + 0.00335 * d
        let p1 = f32::from(self.pulse1.output());
        let p2 = f32::from(self.pulse2.output());
        let t = f32::from(self.triangle.output());
        let n = f32::from(self.noise.output());
        let d = f32::from(self.dmc.output());

        let pulse_out = 0.007_52 * (p1 + p2);
        let tnd_out = 0.008_51 * t + 0.004_94 * n + 0.003_35 * d;
        pulse_out + tnd_out
    }
}
