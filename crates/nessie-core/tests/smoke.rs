//! End-to-end headless smoke test (spec §8.3).
//!
//! Loads the homebrew `smoke.nes` fixture, runs 120 NTSC frames through the
//! public [`Nes`] facade, and asserts:
//!
//! 1. The SHA-1 of the final framebuffer matches a committed expected hex
//!    digest. The fixture deliberately never enables PPU rendering, so the
//!    PPU outputs the backdrop colour (palette entry `$3F00`, still `$00` =
//!    grey at power-on) for every pixel on all platforms; this gives the
//!    test a stable hash that does not depend on the host operating system,
//!    CPU architecture, or Rust release.
//! 2. The APU produced non-zero audio samples while the ROM ran. The
//!    fixture enables pulse channel 1 with a non-trivial duty / volume so
//!    the mixer emits non-zero amplitude.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use nessie_core::Nes;
use sha1::{Digest, Sha1};

const SMOKE_ROM: &[u8] = include_bytes!("fixtures/smoke.nes");

/// SHA-1 of a 256×240 RGBA framebuffer filled with NES colour `$00`
/// (`84, 84, 84, 255`). The smoke ROM never enables rendering, so the PPU
/// shows the power-on backdrop colour on every pixel.
const EXPECTED_FRAME_SHA1: &str = "737081037b395e1b2b2f525d9f37f33320c45a9f";

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let d = hasher.finalize();
    let mut s = String::with_capacity(40);
    for b in d {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

#[test]
fn smoke_rom_runs_120_frames_with_stable_framebuffer_and_audio() {
    let mut nes = Nes::from_ines(SMOKE_ROM).expect("smoke.nes must parse");
    let info = nes.cartridge_info();
    assert_eq!(info.mapper, 0);
    assert!(!info.has_battery);

    let mut samples = Vec::new();
    let mut nonzero_sample_seen = false;

    for _ in 0..120 {
        nes.step_frame();
        let mut frame_audio = Vec::new();
        nes.drain_audio(&mut frame_audio);
        if frame_audio.iter().any(|s| s.abs() > 0.0) {
            nonzero_sample_seen = true;
        }
        samples.append(&mut frame_audio);
    }

    let actual = sha1_hex(nes.framebuffer());
    assert_eq!(
        actual, EXPECTED_FRAME_SHA1,
        "framebuffer SHA-1 mismatch after 120 frames"
    );
    assert!(
        !samples.is_empty(),
        "APU produced no audio samples during 120 frames"
    );
    assert!(
        nonzero_sample_seen,
        "APU produced only silence — pulse 1 should be audible"
    );
}
