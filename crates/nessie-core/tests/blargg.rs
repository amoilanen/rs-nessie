//! Opt-in accuracy suite: run blargg's public NES test ROMs end to end.
//!
//! The ROMs are not vendored (the collection carries no explicit licence);
//! fetch the pinned copy first, then run the ignored tests:
//!
//! ```text
//! scripts/fetch-test-roms.sh
//! cargo test -p nessie-core --release --test blargg -- --ignored --nocapture
//! ```
//!
//! `NES_TEST_ROMS` overrides the ROM directory (default:
//! `<workspace>/target/nes-test-roms`).
//!
//! Every ROM reports its verdict through blargg's `$6000` protocol (see
//! [`common::run_blargg`]), including ROMs that ask for the console RESET
//! button to be pressed mid-test.
//!
//! [`MUST_PASS`] is the regression gate: each of these ROMs passes today and
//! must keep passing. [`KNOWN_FAILURES`] documents the remaining gaps —
//! sub-instruction timing races (exact NMI/IRQ polling and vblank-flag
//! suppression windows, interrupt hijacking), PPU open-bus decay and the
//! MMC3 revision-A IRQ variant — which commercial games essentially never
//! depend on. Those are run and reported but do not fail the suite; when one
//! starts passing, move it to [`MUST_PASS`].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::PathBuf;

use common::{run_blargg, BlarggResult};

/// Generous upper bound: the slowest ROM here (`instr_timing`) needs ~25 s
/// of emulated time.
const MAX_FRAMES: u32 = 60 * 60;

const MUST_PASS: &[&str] = &[
    // CPU: every official + unofficial opcode, addressing-mode quirks.
    "instr_test-v5/rom_singles/01-basics.nes",
    "instr_test-v5/rom_singles/02-implied.nes",
    "instr_test-v5/rom_singles/03-immediate.nes",
    "instr_test-v5/rom_singles/04-zero_page.nes",
    "instr_test-v5/rom_singles/05-zp_xy.nes",
    "instr_test-v5/rom_singles/06-absolute.nes",
    "instr_test-v5/rom_singles/07-abs_xy.nes",
    "instr_test-v5/rom_singles/08-ind_x.nes",
    "instr_test-v5/rom_singles/09-ind_y.nes",
    "instr_test-v5/rom_singles/10-branches.nes",
    "instr_test-v5/rom_singles/11-stack.nes",
    "instr_test-v5/rom_singles/12-jmp_jsr.nes",
    "instr_test-v5/rom_singles/13-rts.nes",
    "instr_test-v5/rom_singles/14-rti.nes",
    "instr_test-v5/rom_singles/15-brk.nes",
    "instr_test-v5/rom_singles/16-special.nes",
    // CPU: wrap-around, dummy reads, branch timing, IRQ latency.
    "instr_misc/rom_singles/01-abs_x_wrap.nes",
    "instr_misc/rom_singles/02-branch_wrap.nes",
    "instr_misc/rom_singles/03-dummy_reads.nes",
    "instr_misc/rom_singles/04-dummy_reads_apu.nes",
    "instr_timing/rom_singles/2-branch_timing.nes",
    "cpu_interrupts_v2/rom_singles/1-cli_latency.nes",
    "cpu_exec_space/test_cpu_exec_space_ppuio.nes",
    // PPU: vblank flag / NMI basics, odd-frame dot skip, OAM reads.
    "ppu_vbl_nmi/rom_singles/01-vbl_basics.nes",
    "ppu_vbl_nmi/rom_singles/03-vbl_clear_time.nes",
    "ppu_vbl_nmi/rom_singles/04-nmi_control.nes",
    "ppu_vbl_nmi/rom_singles/09-even_odd_frames.nes",
    "oam_read/oam_read.nes",
    // APU: length counters, frame counter + IRQ timing, DMC.
    "apu_test/rom_singles/1-len_ctr.nes",
    "apu_test/rom_singles/2-len_table.nes",
    "apu_test/rom_singles/3-irq_flag.nes",
    "apu_test/rom_singles/4-jitter.nes",
    "apu_test/rom_singles/5-len_timing.nes",
    "apu_test/rom_singles/6-irq_flag_timing.nes",
    "apu_test/rom_singles/7-dmc_basics.nes",
    "apu_test/rom_singles/8-dmc_rates.nes",
    // Console RESET button behaviour (CPU registers, RAM, APU state).
    "apu_reset/4015_cleared.nes",
    "apu_reset/4017_timing.nes",
    "apu_reset/4017_written.nes",
    "apu_reset/irq_flag_cleared.nes",
    "apu_reset/len_ctrs_enabled.nes",
    "apu_reset/works_immediately.nes",
    "cpu_reset/ram_after_reset.nes",
    "cpu_reset/registers.nes",
    // MMC3 scanline counter (A12 clocking, reload, IRQ).
    "mmc3_test_2/rom_singles/1-clocking.nes",
    "mmc3_test_2/rom_singles/2-details.nes",
    "mmc3_test_2/rom_singles/3-A12_clocking.nes",
    "mmc3_test_2/rom_singles/5-MMC3.nes",
];

const KNOWN_FAILURES: &[&str] = &[
    "instr_timing/rom_singles/1-instr_timing.nes",
    "cpu_interrupts_v2/rom_singles/2-nmi_and_brk.nes",
    "cpu_interrupts_v2/rom_singles/3-nmi_and_irq.nes",
    "cpu_interrupts_v2/rom_singles/4-irq_and_dma.nes",
    "cpu_interrupts_v2/rom_singles/5-branch_delays_irq.nes",
    "cpu_exec_space/test_cpu_exec_space_apu.nes",
    "ppu_vbl_nmi/rom_singles/02-vbl_set_time.nes",
    "ppu_vbl_nmi/rom_singles/05-nmi_timing.nes",
    "ppu_vbl_nmi/rom_singles/06-suppression.nes",
    "ppu_vbl_nmi/rom_singles/07-nmi_on_timing.nes",
    "ppu_vbl_nmi/rom_singles/08-nmi_off_timing.nes",
    "ppu_vbl_nmi/rom_singles/10-even_odd_timing.nes",
    "ppu_open_bus/ppu_open_bus.nes",
    "mmc3_test_2/rom_singles/4-scanline_timing.nes",
    // Tests the older MMC3 revision; the emulated chip is revision B.
    "mmc3_test_2/rom_singles/6-MMC3_alt.nes",
];

fn rom_dir() -> PathBuf {
    let dir = std::env::var_os("NES_TEST_ROMS").map_or_else(
        || PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/nes-test-roms"),
        PathBuf::from,
    );
    assert!(
        dir.is_dir(),
        "test ROMs not found at {} — run scripts/fetch-test-roms.sh (or set NES_TEST_ROMS)",
        dir.display()
    );
    dir
}

fn run(dir: &std::path::Path, rel: &str) -> BlarggResult {
    let path = dir.join(rel);
    let rom = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    run_blargg(&rom, MAX_FRAMES)
}

#[test]
#[ignore = "needs public test ROMs: run scripts/fetch-test-roms.sh, then `-- --ignored`"]
fn blargg_regression_suite() {
    let dir = rom_dir();

    let mut failures = Vec::new();
    for rel in MUST_PASS {
        let result = run(&dir, rel);
        println!("{:<55} {result:?}", rel);
        if result != BlarggResult::Passed {
            failures.push(format!("{rel}: {result:?}"));
        }
    }

    let mut now_passing = Vec::new();
    for rel in KNOWN_FAILURES {
        let result = run(&dir, rel);
        println!("{:<55} (known gap) {result:?}", rel);
        if result == BlarggResult::Passed {
            now_passing.push(*rel);
        }
    }
    if !now_passing.is_empty() {
        println!("Now passing — promote to MUST_PASS: {now_passing:?}");
    }

    assert!(
        failures.is_empty(),
        "{} of {} must-pass test ROMs regressed:\n{}",
        failures.len(),
        MUST_PASS.len(),
        failures.join("\n")
    );
}
