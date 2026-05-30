//! Unit tests for the APU.

#![allow(clippy::unwrap_used, clippy::panic, clippy::expect_used)]

use super::*;

// -----------------------------------------------------------------------
// Pulse length counter.
// -----------------------------------------------------------------------

/// The pulse length counter must decrement on the half-frame ticks of the
/// frame counter (step 2 and step 4 in 4-step mode), and freeze when the
/// halt flag is set.
#[test]
fn pulse_length_counter_decrements_on_half_frame_ticks() {
    let mut apu = Apu::new();
    // 4-step mode, IRQ inhibited so the test isn't polluted by IRQs.
    apu.write_register(0x4017, 0x40);
    // Enable pulse 1.
    apu.write_register(0x4015, 0x01);
    // $4000: duty 0, halt = 0, constant volume, vol = 0.
    apu.write_register(0x4000, 0b0001_0000);
    // $4002/$4003: timer = 0x100, length load = 0 → length table[0] = 10.
    apu.write_register(0x4002, 0x00);
    apu.write_register(0x4003, 0x01); // length-idx (v>>3) = 0; timer hi = 1

    assert_eq!(apu.pulse1.length.value(), 10);

    // Step just past the first half-frame event (14913 CPU cycles).
    apu.step(14913);
    assert_eq!(apu.pulse1.length.value(), 9, "decrement after step 2");

    // Step to the next half-frame event (29829 CPU cycles total).
    apu.step(29829 - 14913);
    assert_eq!(apu.pulse1.length.value(), 8, "decrement after step 4");
}

#[test]
fn pulse_length_counter_halt_freezes_count() {
    let mut apu = Apu::new();
    apu.write_register(0x4017, 0x40); // 4-step, IRQ off
    apu.write_register(0x4015, 0x01); // enable pulse 1
                                      // halt = 1, constant volume, vol = 0.
    apu.write_register(0x4000, 0b0011_0000);
    apu.write_register(0x4002, 0x00);
    apu.write_register(0x4003, 0x01); // length 10

    assert_eq!(apu.pulse1.length.value(), 10);

    // Even after multiple full frames the counter must not move.
    apu.step(29829 * 3);
    assert_eq!(apu.pulse1.length.value(), 10, "halt flag freezes the count");
}

#[test]
fn disabling_pulse_via_4015_clears_length() {
    let mut apu = Apu::new();
    apu.write_register(0x4017, 0x40);
    apu.write_register(0x4015, 0x01);
    apu.write_register(0x4000, 0b0001_0000);
    apu.write_register(0x4002, 0x00);
    apu.write_register(0x4003, 0x01);
    assert!(apu.pulse1.length.active());

    // Disabling the channel via $4015 must clear the length counter.
    apu.write_register(0x4015, 0x00);
    assert!(!apu.pulse1.length.active());
}

// -----------------------------------------------------------------------
// $4015 status read clears the frame interrupt flag.
// -----------------------------------------------------------------------

#[test]
fn status_read_clears_frame_interrupt() {
    let mut apu = Apu::new();
    // 4-step mode, IRQ enabled.
    apu.write_register(0x4017, 0x00);
    // Run a full frame so the step-4 IRQ fires.
    apu.step(29830);
    assert!(apu.frame_irq, "frame IRQ should be set after one frame");
    assert!(apu.irq_pending());

    let status = apu.read_status();
    assert_ne!(status & 0x40, 0, "status bit 6 reflects frame IRQ");
    assert!(
        !apu.frame_irq,
        "reading status must clear the frame IRQ flag"
    );
    assert!(!apu.irq_pending());
}

#[test]
fn frame_irq_inhibit_suppresses_interrupt() {
    let mut apu = Apu::new();
    apu.write_register(0x4017, 0x40); // inhibit IRQ
    apu.step(29830);
    assert!(!apu.frame_irq);
    assert!(!apu.irq_pending());
}

// -----------------------------------------------------------------------
// Noise LFSR sequence.
// -----------------------------------------------------------------------

/// 16-step golden trace of the LFSR starting from the power-on value `1`
/// in mode 0 (feedback = bit0 XOR bit1).
#[test]
fn noise_lfsr_mode0_golden_trace() {
    let mut noise = Noise::new();
    assert_eq!(noise.lfsr, 1);

    let expected = [
        0x4000u16, 0x2000, 0x1000, 0x0800, 0x0400, 0x0200, 0x0100, 0x0080, 0x0040, 0x0020, 0x0010,
        0x0008, 0x0004, 0x0002, 0x4001, 0x6000,
    ];

    let mut actual = [0u16; 16];
    for slot in actual.iter_mut() {
        noise.shift_lfsr();
        *slot = noise.lfsr;
    }
    assert_eq!(actual, expected, "mode-0 LFSR sequence mismatch");
}

#[test]
fn noise_lfsr_mode1_differs_from_mode0() {
    // Mode 1 (bit-6 tap) diverges from mode 0 at the very first shift
    // whenever bits 1 and 6 of the LFSR differ. Seed the LFSR with such a
    // pattern and confirm.
    let mut a = Noise::new();
    let mut b = Noise::new();
    a.lfsr = 0b100_0010; // bit1=1, bit6=1 → identical first shift
    b.lfsr = 0b000_0010; // bit1=1, bit6=0 → different first shift
    b.mode = true;

    a.shift_lfsr();
    b.shift_lfsr();
    assert_ne!(a.lfsr, b.lfsr, "mode-1 feedback must diverge from mode 0");
}

// -----------------------------------------------------------------------
// Triangle linear counter.
// -----------------------------------------------------------------------

#[test]
fn triangle_linear_counter_reloads_with_control_flag_set() {
    let mut apu = Apu::new();
    apu.write_register(0x4017, 0x40); // 4-step, IRQ off
    apu.write_register(0x4015, 0x04); // enable triangle
                                      // $4008: control flag = 1, reload value = 0x3F.
    apu.write_register(0x4008, 0x80 | 0x3F);
    // Trigger reload-flag-set with $400B write (length index 1 →
    // length-table[1] = 254 so length stays active).
    apu.write_register(0x400A, 0x00);
    apu.write_register(0x400B, 0x08); // length-idx (v>>3)=1, timer hi=0
    assert_eq!(apu.triangle.linear_counter, 0);

    // One full frame triggers four quarter-frame clocks.
    apu.step(29830);
    // With control_flag = 1, the linear-reload flag is never cleared, so
    // every quarter-frame clock keeps reloading the counter to 0x3F.
    assert_eq!(apu.triangle.linear_counter, 0x3F);
}

#[test]
fn triangle_linear_counter_decrements_when_control_clear() {
    let mut apu = Apu::new();
    apu.write_register(0x4017, 0x40); // 4-step, IRQ off
    apu.write_register(0x4015, 0x04); // enable triangle
                                      // First set the control flag and trigger the reload flag.
    apu.write_register(0x4008, 0x80 | 0x05);
    apu.write_register(0x400A, 0x00);
    apu.write_register(0x400B, 0x08);

    // Clear the control flag *before* the first quarter-frame clock.
    // After the first reload the linear-reload flag will be cleared and
    // subsequent clocks must decrement the counter.
    apu.write_register(0x4008, 0x05);

    // Advance to the first quarter-frame: linear reload happens, then the
    // reload flag is cleared (control flag is now 0).
    apu.step(7457);
    assert_eq!(apu.triangle.linear_counter, 5);

    // Next quarter-frame (step 2 of 4-step mode): counter -= 1.
    apu.step(14913 - 7457);
    assert_eq!(apu.triangle.linear_counter, 4);

    // Next quarter-frame (step 3): counter -= 1.
    apu.step(22371 - 14913);
    assert_eq!(apu.triangle.linear_counter, 3);
}

// -----------------------------------------------------------------------
// drain_samples produces the expected count.
// -----------------------------------------------------------------------

#[test]
fn drain_samples_yields_expected_count_at_44100() {
    let mut apu = Apu::with_sample_rate(44_100);
    let cycles = NES_CPU_HZ; // exactly one second of audio
    apu.step(cycles);

    let mut out = Vec::new();
    apu.drain_samples(&mut out);

    let expected = (u64::from(cycles) * 44_100) / u64::from(NES_CPU_HZ);
    let diff = (out.len() as i64 - expected as i64).abs();
    assert!(
        diff <= 1,
        "expected ~{expected} samples, got {} (diff {diff})",
        out.len()
    );
}

#[test]
fn drain_samples_yields_expected_count_at_48000() {
    let mut apu = Apu::with_sample_rate(48_000);
    let cycles = 100_000u32;
    apu.step(cycles);

    let mut out = Vec::new();
    apu.drain_samples(&mut out);

    let expected = (u64::from(cycles) * 48_000) / u64::from(NES_CPU_HZ);
    let diff = (out.len() as i64 - expected as i64).abs();
    assert!(
        diff <= 1,
        "expected ~{expected} samples, got {} (diff {diff})",
        out.len()
    );
}

#[test]
fn drain_samples_empties_the_buffer() {
    let mut apu = Apu::with_sample_rate(44_100);
    apu.step(100_000);

    let mut out = Vec::new();
    apu.drain_samples(&mut out);
    assert!(!out.is_empty());
    assert_eq!(apu.buffered_samples(), 0);

    let mut out2 = Vec::new();
    apu.drain_samples(&mut out2);
    assert!(out2.is_empty(), "draining twice yields no new samples");
}

// -----------------------------------------------------------------------
// Bonus: register write smoke tests.
// -----------------------------------------------------------------------

#[test]
fn writing_unused_registers_does_not_panic() {
    let mut apu = Apu::new();
    for addr in [0x4009u16, 0x400D, 0x4014, 0x4016, 0x4018, 0x401F] {
        apu.write_register(addr, 0xFF);
    }
}

#[test]
fn dmc_irq_flag_set_when_sample_exhausts_non_loop() {
    let mut apu = Apu::new();
    // DMC IRQ enabled, no loop, fastest rate.
    apu.write_register(0x4010, 0x80 | 0x0F);
    // Small sample length: $4013 = 0 → 1 byte.
    apu.write_register(0x4013, 0x00);
    // Enable DMC channel: copies length into bytes_remaining (= 1).
    apu.write_register(0x4015, 0x10);
    assert_eq!(apu.dmc.bytes_remaining, 1);

    // Step enough cycles for the timer to fire and consume the byte.
    // Rate index 15 = 54 CPU cycles; allow some slack.
    apu.step(200);
    assert_eq!(apu.dmc.bytes_remaining, 0);
    assert!(apu.dmc.irq_flag, "DMC IRQ should fire when sample exhausts");
    assert!(apu.irq_pending());
}
