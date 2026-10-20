//! End-to-end test: play a real homebrew game through the public [`Nes`]
//! facade.
//!
//! `fixtures/nes15-NTSC.nes` is *nes15* by Mathew Brenaman — an NROM
//! implementation of the Fifteen Puzzle with background music, released
//! under a BSD-2-Clause licence (see `fixtures/README.md`). The test scripts
//! a short play session, exactly like a player at the keyboard would:
//!
//! 1. Boot to the (silent) title screen.
//! 2. Press **Start** → the shuffled puzzle board appears with a sprite
//!    selection cursor and a `MOVES- 0` counter, and the music starts.
//! 3. Press **Right** → the cursor moves one tile to the right.
//! 4. Press **A** → the selected tile slides into the hole and the move
//!    counter becomes `1`.
//!
//! Each milestone is checked both semantically (screen changed in the right
//! place, enough colours, audible audio) and against a golden framebuffer
//! SHA-1 so that any rendering/CPU/timing regression is caught. The golden
//! values were captured from this emulator after visually verifying the
//! frames; if an intentional accuracy change alters them, re-verify the
//! screenshots and update the constants.
//!
//! [`Nes`]: nessie_core::Nes

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::{press, rms, Player1Script, Press};
use nessie_core::Button;

const NES15: &[u8] = include_bytes!("fixtures/nes15-NTSC.nes");

/// Frame at which the title screen is fully drawn.
const TITLE_FRAME: u32 = 120;
/// Start pressed at frame 150; the puzzle board is up well before 290.
const BOARD_FRAME: u32 = 290;
/// Right pressed at frame 300; cursor has moved by 340.
const CURSOR_FRAME: u32 = 340;
/// A pressed at frame 350; the slide animation has finished by 420.
const SLID_FRAME: u32 = 420;

const SCRIPT: [Press; 3] = [
    press(150, 5, Button::Start),
    press(300, 5, Button::Right),
    press(350, 5, Button::A),
];

// Golden framebuffer digests (RGBA8, 256×240) at the milestone frames.
const TITLE_SHA1: &str = "301edc121960d61738fa6f3dace88ced29686d3e";
const BOARD_SHA1: &str = "6be3d4f0954db6df9e03a7e0cdf3ab004eb1929d";
const CURSOR_SHA1: &str = "c2f509a60db56345d874a0202918472b6a2868bf";
const SLID_SHA1: &str = "c71aa7a25b13da7f524d676f4cc9634f382128fe";

/// The `MOVES- n` counter in the bottom-right corner of the board.
fn moves_counter(run: &Player1Script) -> Vec<u8> {
    run.region(120, 208, 128, 16)
}

/// The board's top-left 2×2 tiles: the cursor starts on tile (0,0), the
/// hole is at (1,1) and tile (1,0) is the one that slides.
fn top_left_tiles(run: &Player1Script) -> Vec<u8> {
    run.region(64, 40, 64, 64)
}

#[test]
fn nes15_boots_and_plays_through_a_move() {
    let mut run = Player1Script::new(NES15, &SCRIPT);

    // --- Title screen ---------------------------------------------------
    run.run_until(TITLE_FRAME);
    assert!(
        run.distinct_colours() >= 4,
        "title screen should be drawn with several colours, got {}",
        run.distinct_colours()
    );
    let title = run.frame_hash();
    let title_samples = run.audio.len();

    // --- Start → puzzle board ------------------------------------------
    run.run_until(BOARD_FRAME);
    let board = run.frame_hash();
    assert_ne!(board, title, "pressing Start must leave the title screen");
    let music = rms(&run.audio[title_samples..]);
    assert!(
        music > 0.01,
        "in-game music (pulse/triangle/noise) should be audible, rms = {music}"
    );
    let moves_before = moves_counter(&run);
    let tiles_before = top_left_tiles(&run);

    // --- Right → cursor moves --------------------------------------------
    run.run_until(CURSOR_FRAME);
    let cursor = run.frame_hash();
    assert_ne!(cursor, board, "pressing Right must move the cursor");
    assert_eq!(
        moves_counter(&run),
        moves_before,
        "moving the cursor is not a move"
    );

    // --- A → tile slides, counter increments ----------------------------
    run.run_until(SLID_FRAME);
    let slid = run.frame_hash();
    assert_ne!(
        top_left_tiles(&run),
        tiles_before,
        "pressing A must slide the selected tile into the hole"
    );
    assert_ne!(
        moves_counter(&run),
        moves_before,
        "the MOVES counter must change after sliding a tile"
    );

    // --- Golden frames ----------------------------------------------------
    let actual = [
        ("title", title.as_str(), TITLE_SHA1),
        ("board", board.as_str(), BOARD_SHA1),
        ("cursor", cursor.as_str(), CURSOR_SHA1),
        ("slid", slid.as_str(), SLID_SHA1),
    ];
    for (name, got, want) in actual {
        assert_eq!(got, want, "golden `{name}` frame changed");
    }
}

#[test]
fn nes15_run_is_deterministic() {
    let hashes = || {
        let mut run = Player1Script::new(NES15, &SCRIPT);
        run.run_until(SLID_FRAME);
        (run.frame_hash(), run.audio.len())
    };
    assert_eq!(
        hashes(),
        hashes(),
        "same input script must give the same output"
    );
}

#[test]
fn nes15_produces_audio_at_the_host_rate() {
    let mut run = Player1Script::new(NES15, &[]);
    run.run_until(60);
    // 60 NTSC frames ≈ 0.998 s of audio at 44.1 kHz.
    let n = run.audio.len();
    assert!(
        (43_500..=44_500).contains(&n),
        "expected ≈44 100 samples for 60 frames, got {n}"
    );
    assert!(run.audio.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
}
