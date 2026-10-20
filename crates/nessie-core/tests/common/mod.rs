//! Shared helpers for the ROM-driven end-to-end tests.
//!
//! Each integration test binary includes this module with `mod common;`, so
//! not every helper is used by every binary.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use nessie_core::{Button, Nes, Player};
use sha1::{Digest, Sha1};

/// Width of the NES picture in pixels.
pub const WIDTH: usize = 256;

/// Lowercase hex SHA-1 of `bytes`.
pub fn sha1_hex(bytes: &[u8]) -> String {
    let digest = Sha1::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// A button held on player 1's controller for `frames` frames starting at
/// frame `at`.
#[derive(Debug, Clone, Copy)]
pub struct Press {
    pub at: u32,
    pub frames: u32,
    pub button: Button,
}

/// Shorthand constructor for [`Press`].
pub const fn press(at: u32, frames: u32, button: Button) -> Press {
    Press { at, frames, button }
}

/// Drives a [`Nes`] frame by frame while replaying a scripted input
/// sequence, the way a player (or the desktop runtime) would.
pub struct Player1Script {
    pub nes: Nes,
    presses: Vec<Press>,
    frame: u32,
    /// Every audio sample produced so far.
    pub audio: Vec<f32>,
}

impl Player1Script {
    pub fn new(rom: &[u8], presses: &[Press]) -> Self {
        Self {
            nes: Nes::from_ines(rom).expect("ROM must load"),
            presses: presses.to_vec(),
            frame: 0,
            audio: Vec::new(),
        }
    }

    /// Number of frames emulated so far.
    pub fn frame(&self) -> u32 {
        self.frame
    }

    /// Emulate frames until `frame()` reaches `target`.
    pub fn run_until(&mut self, target: u32) {
        while self.frame < target {
            for button in ALL_BUTTONS {
                let held = self.presses.iter().any(|p| {
                    p.button == button && self.frame >= p.at && self.frame < p.at + p.frames
                });
                self.nes.set_button(Player::One, button, held);
            }
            self.nes.step_frame();
            self.nes.drain_audio(&mut self.audio);
            self.frame += 1;
        }
    }

    /// SHA-1 of the last completed frame.
    pub fn frame_hash(&self) -> String {
        sha1_hex(self.nes.framebuffer())
    }

    /// Copy of the RGBA pixels in the rectangle `x..x+w`, `y..y+h`.
    pub fn region(&self, x: usize, y: usize, w: usize, h: usize) -> Vec<u8> {
        let fb = self.nes.framebuffer();
        let mut out = Vec::with_capacity(w * h * 4);
        for row in y..y + h {
            let start = (row * WIDTH + x) * 4;
            out.extend_from_slice(&fb[start..start + w * 4]);
        }
        out
    }

    /// Number of distinct colours in the last completed frame.
    pub fn distinct_colours(&self) -> usize {
        let mut seen = std::collections::HashSet::new();
        for px in self.nes.framebuffer().chunks_exact(4) {
            seen.insert([px[0], px[1], px[2]]);
        }
        seen.len()
    }
}

const ALL_BUTTONS: [Button; 8] = [
    Button::A,
    Button::B,
    Button::Select,
    Button::Start,
    Button::Up,
    Button::Down,
    Button::Left,
    Button::Right,
];

/// Root-mean-square amplitude of `samples`.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Outcome of a blargg-style test ROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlarggResult {
    /// `$6000` reported `0`.
    Passed,
    /// `$6000` reported a non-zero result code; `text` is the ROM's report.
    Failed { code: u8, text: String },
    /// The ROM never reported a result within the frame budget.
    TimedOut { text: String },
}

/// Run a ROM that follows blargg's `$6000` result protocol: once the
/// signature `DE B0 61` appears at `$6001-$6003`, `$6000` holds `$80`
/// while running, `$81` when it wants the console RESET button pressed,
/// and the final result code (`0` = pass) otherwise. `$6004..` holds a
/// NUL-terminated text report.
pub fn run_blargg(rom: &[u8], max_frames: u32) -> BlarggResult {
    let mut nes = Nes::from_ines(rom).expect("test ROM must load");
    // Frames to wait after a reset request before "pressing" RESET (the ROM
    // asks to be reset only after its message has been shown).
    const RESET_DELAY: u32 = 10;
    let mut reset_at: Option<u32> = None;
    let mut armed = true;
    for frame in 0..max_frames {
        nes.step_frame();
        let signed = nes.peek_cpu(0x6001) == 0xDE
            && nes.peek_cpu(0x6002) == 0xB0
            && nes.peek_cpu(0x6003) == 0x61;
        if !signed {
            continue;
        }
        let status = nes.peek_cpu(0x6000);
        match status {
            0x80 => armed = true,
            0x81 if armed && reset_at.is_none() => reset_at = Some(frame + RESET_DELAY),
            0 => return BlarggResult::Passed,
            code if code < 0x80 => {
                return BlarggResult::Failed {
                    code,
                    text: blargg_text(&mut nes),
                }
            }
            _ => {}
        }
        if reset_at == Some(frame) {
            nes.reset();
            reset_at = None;
            armed = false;
        }
    }
    BlarggResult::TimedOut {
        text: blargg_text(&mut nes),
    }
}

fn blargg_text(nes: &mut Nes) -> String {
    let mut text = String::new();
    for addr in 0x6004u16..0x7000 {
        let c = nes.peek_cpu(addr);
        if c == 0 {
            break;
        }
        text.push(c as char);
    }
    text.trim().replace('\n', " | ")
}
