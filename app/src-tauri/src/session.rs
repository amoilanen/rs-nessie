//! Host-owned emulator session.
//!
//! [`EmulatorSession`] is the desktop shell's handle to a running emulator
//! game. It owns the [`nessie_runtime::Session`] (which spawns the emulation
//! thread), the [`crate::audio::CpalAudio`] sink that backs OS audio
//! output, and a [`FrameSink`] adapter that forwards rendered framebuffers
//! to the frontend (spec §5.3, §6.1).
//!
//! The session is the single point at which battery-backed PRG-RAM is
//! loaded and persisted (spec §4.3):
//!
//! - On [`EmulatorSession::start`] the caller may pass pre-read `.srm`
//!   bytes; the session forwards them to [`nessie_core::Nes::load_battery`].
//! - On [`EmulatorSession::stop`] the runtime hands back the final
//!   battery snapshot through [`nessie_runtime::BatteryOut`]; the session
//!   writes it to `<saves_dir>/<sha1>.srm` atomically (`.tmp` + `rename`).
//!
//! [`EmulatorSession::stop`] is idempotent — a second call is a no-op.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

use nessie_core::Nes;
use nessie_runtime::{
    AudioSink, BatteryOut, FrameSink, Player as RuntimePlayer, Session, SessionOptions,
};

use crate::audio::{CpalAudio, APU_SAMPLE_RATE};
use crate::error::{AppError, AppResult};
use crate::library::APP_CONFIG_DIR_NAME;

/// Sub-directory under the OS config dir that holds per-ROM `.srm` battery
/// saves (`<config>/dev.rs-nessie/saves/`, spec §4.3).
pub const SAVES_DIR_NAME: &str = "saves";

/// Trait combining [`AudioSink`] with live volume / mute control.
///
/// The host stores its audio sink as `Arc<dyn HostAudio>` so production
/// uses [`CpalAudio`] while tests can substitute a tiny mock that records
/// pushed samples without opening an audio device.
pub trait HostAudio: AudioSink {
    /// Update the master output volume (clamped to `0.0..=1.0`).
    fn set_volume(&self, volume: f32);
    /// Mute or un-mute the output.
    fn set_muted(&self, muted: bool);
}

impl HostAudio for CpalAudio {
    fn set_volume(&self, volume: f32) {
        CpalAudio::set_volume(self, volume);
    }
    fn set_muted(&self, muted: bool) {
        CpalAudio::set_muted(self, muted);
    }
}

/// Identifies which physical player slot an input applies to.
///
/// Mirrors `nessie_runtime::Player` but lives in the host crate so command
/// modules can take a tagged enum without leaking the runtime's
/// representation through Tauri IPC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionPlayer {
    /// Player 1.
    One,
    /// Player 2.
    Two,
}

impl From<SessionPlayer> for RuntimePlayer {
    fn from(p: SessionPlayer) -> Self {
        match p {
            SessionPlayer::One => RuntimePlayer::One,
            SessionPlayer::Two => RuntimePlayer::Two,
        }
    }
}

/// Handle to an active emulation session.
///
/// Constructed by [`EmulatorSession::start`] (production: opens a real cpal
/// stream) or [`EmulatorSession::start_with`] (tests: caller supplies its
/// own audio/frame sinks).
pub struct EmulatorSession {
    /// The runtime session driving the emulation thread.
    runtime: Session,
    /// Audio sink the runtime is pushing samples into; held here so the
    /// host can adjust volume / mute and so the cpal stream is kept alive
    /// for the lifetime of the session.
    audio: Arc<dyn HostAudio>,
    /// Slot the runtime fills with the final battery snapshot before the
    /// emulation thread exits.
    battery_out: BatteryOut,
    /// Absolute path where this cartridge's `.srm` save should be written
    /// (resolved at start time so [`stop`](Self::stop) is a pure local
    /// operation).
    save_path: Option<PathBuf>,
    /// `true` once [`stop`](Self::stop) has been called and acted upon.
    /// Kept around so the second call is an unambiguous no-op.
    stopped: bool,
    /// Cartridge SHA-1 (kept for diagnostics; the `save_path` is what
    /// actually gets written).
    sha1: String,
}

impl std::fmt::Debug for EmulatorSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmulatorSession")
            .field("sha1", &self.sha1)
            .field("save_path", &self.save_path)
            .field("stopped", &self.stopped)
            .finish()
    }
}

impl EmulatorSession {
    /// Start a session backed by a real cpal audio device.
    ///
    /// `rom_bytes` is the iNES blob to load. `saves_dir` is the directory
    /// `<config>/dev.rs-nessie/saves/` that will receive the `.srm` file on
    /// stop; the session also tries to load a pre-existing
    /// `<saves_dir>/<sha1>.srm` and feed it to the emulator core.
    ///
    /// `frame_sink` is the consumer of rendered frames. In production this
    /// is a thin adapter around a Tauri `Channel<FrameMessage>` (wired in
    /// the IPC commands step); tests pass a mock recorder.
    pub fn start<F>(
        rom_bytes: &[u8],
        save_bytes: Option<Vec<u8>>,
        saves_dir: PathBuf,
        frame_sink: Arc<F>,
    ) -> AppResult<Self>
    where
        F: FrameSink + 'static,
    {
        let audio: Arc<dyn HostAudio> = Arc::new(CpalAudio::new(APU_SAMPLE_RATE)?);
        Self::start_with(rom_bytes, save_bytes, saves_dir, audio, frame_sink)
    }

    /// Start a session with a caller-supplied audio sink.
    ///
    /// Used by tests (mock sinks, no audio device required) and by future
    /// host integrations that want to swap the audio backend without
    /// touching the rest of the session machinery.
    pub fn start_with<F>(
        rom_bytes: &[u8],
        save_bytes: Option<Vec<u8>>,
        saves_dir: PathBuf,
        audio: Arc<dyn HostAudio>,
        frame_sink: Arc<F>,
    ) -> AppResult<Self>
    where
        F: FrameSink + 'static,
    {
        let mut nes = Nes::from_ines(rom_bytes).map_err(map_core_error)?;
        let info = nes.cartridge_info();
        let sha1 = info.sha1.clone();
        let has_battery = info.has_battery;

        // Resolve the path *now* so a missing config dir surfaces here
        // (before the emu thread spawns) and so `stop()` can write atomically
        // without re-resolving anything.
        let save_path = if has_battery {
            Some(saves_dir.join(format!("{sha1}.srm")))
        } else {
            None
        };

        // Load battery save bytes into the core *before* spawning the
        // emulation thread. `load_battery` is a no-op when the size does
        // not match the cartridge's PRG-RAM, so a stale save never panics.
        if has_battery {
            if let Some(bytes) = save_bytes {
                nes.load_battery(&bytes);
            }
        }

        let battery_out: BatteryOut = Arc::new(Mutex::new(None));
        let options = SessionOptions {
            battery_out: Some(Arc::clone(&battery_out)),
            ..Default::default()
        };

        // Adapt the host-facing `Arc<dyn HostAudio>` into the
        // `Arc<A: AudioSink + 'static>` shape that the runtime expects.
        // A tiny delegating wrapper keeps us on safe stable Rust (trait
        // upcasting is still nightly-only).
        let audio_for_runtime = Arc::new(AudioSinkAdapter(Arc::clone(&audio)));
        let runtime = Session::start_with(
            nes,
            audio_for_runtime,
            frame_sink,
            options,
            nessie_runtime::SystemClock,
        );

        Ok(Self {
            runtime,
            audio,
            battery_out,
            save_path,
            stopped: false,
            sha1,
        })
    }

    /// Forward a button state change to the runtime.
    pub fn set_button(&self, player: SessionPlayer, button: nessie_core::Button, pressed: bool) {
        self.runtime.set_button(player.into(), button, pressed);
    }

    /// Pause or resume the audio stream.
    ///
    /// Implemented as a mute toggle: the emulation thread keeps running
    /// (so input remains responsive and the frame counter stays sane), but
    /// audio output is silenced. A future iteration can wire a real
    /// "freeze the emu thread" path; spec §6.1 deliberately keeps that out
    /// of scope for the initial release.
    pub fn set_paused(&self, paused: bool) {
        self.audio.set_muted(paused);
    }

    /// Update the output volume (`0.0..=1.0`, clamped).
    pub fn set_volume(&self, volume: f32) {
        self.audio.set_volume(volume);
    }

    /// Mute or un-mute the output.
    pub fn set_muted(&self, muted: bool) {
        self.audio.set_muted(muted);
    }

    /// Stop the emulation thread, persist the battery save (if any), and
    /// release the cpal audio device. Idempotent.
    pub fn stop(&mut self) -> AppResult<()> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        self.runtime.stop();
        // The runtime guarantees that by the time `stop` returns the emu
        // thread has exited and `battery_out` reflects the final snapshot.
        let snapshot = self.battery_out.lock().take();
        if let (Some(path), Some(bytes)) = (self.save_path.as_ref(), snapshot) {
            if let Err(err) = write_atomic(path, &bytes) {
                log::error!("failed to persist battery save to {path:?}: {err:?}");
                return Err(err);
            }
        }
        Ok(())
    }

    /// SHA-1 of the loaded cartridge (lowercase hex, 40 chars).
    #[must_use]
    pub fn sha1(&self) -> &str {
        &self.sha1
    }

    /// Battery-save path for this cartridge, if any (`None` for
    /// non-battery cartridges).
    #[must_use]
    pub fn save_path(&self) -> Option<&Path> {
        self.save_path.as_deref()
    }
}

impl Drop for EmulatorSession {
    fn drop(&mut self) {
        // Best-effort cleanup: persist the battery save and release the
        // audio device even if the host forgot to call `stop()`.
        if !self.stopped {
            if let Err(err) = self.stop() {
                log::error!("EmulatorSession drop failed to stop cleanly: {err:?}");
            }
        }
    }
}

/// Resolve the default saves directory
/// (`<OS config dir>/dev.rs-nessie/saves/`).
pub fn default_saves_dir() -> AppResult<PathBuf> {
    let dir = dirs::config_dir()
        .ok_or_else(|| AppError::Io("could not resolve OS user config directory".into()))?;
    Ok(dir.join(APP_CONFIG_DIR_NAME).join(SAVES_DIR_NAME))
}

/// Read an existing `.srm` for `sha1` from `saves_dir`, returning `None` if
/// the file does not exist.
pub fn load_save_bytes(saves_dir: &Path, sha1: &str) -> AppResult<Option<Vec<u8>>> {
    let path = saves_dir.join(format!("{sha1}.srm"));
    match fs::read(&path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(AppError::from(err)),
    }
}

/// Write `bytes` to `path` atomically (`.tmp` sibling + `rename`).
fn write_atomic(path: &Path, bytes: &[u8]) -> AppResult<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let mut tmp_os: OsString = path.as_os_str().to_owned();
    tmp_os.push(".tmp");
    let tmp = PathBuf::from(tmp_os);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Map `nessie_core::CoreError` into the host's [`AppError`].
fn map_core_error(err: nessie_core::CoreError) -> AppError {
    match err {
        nessie_core::CoreError::InvalidRom(msg) => AppError::InvalidRom(msg),
        nessie_core::CoreError::UnsupportedMapper(n) => AppError::UnsupportedMapper(n),
        nessie_core::CoreError::Io(msg) => AppError::Io(msg),
    }
}

/// Delegating wrapper that exposes a `Arc<dyn HostAudio>` as the
/// [`AudioSink`] trait the runtime consumes.
///
/// Trait upcasting (`Arc<dyn HostAudio>` → `Arc<dyn AudioSink>`) is
/// unstable on Rust 1.78, so we route through a tiny adapter instead. The
/// runtime's monomorphized API takes `Arc<A: AudioSink + 'static>` — handing
/// it `Arc<AudioSinkAdapter>` keeps everything on safe stable Rust.
struct AudioSinkAdapter(Arc<dyn HostAudio>);

impl AudioSink for AudioSinkAdapter {
    fn push_samples(&self, samples: &[f32]) {
        self.0.push_samples(samples);
    }
    fn sample_rate(&self) -> u32 {
        self.0.sample_rate()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use parking_lot::Mutex as PlMutex;
    use tempfile::tempdir;

    use nessie_runtime::{AudioSink, FrameSink, FRAME_BYTES};

    use super::*;

    /// Mock audio sink: records all pushed samples for later inspection.
    struct MockAudio {
        samples: PlMutex<Vec<f32>>,
        volume: PlMutex<f32>,
        muted: PlMutex<bool>,
    }

    impl MockAudio {
        fn new() -> Self {
            Self {
                samples: PlMutex::new(Vec::new()),
                volume: PlMutex::new(1.0),
                muted: PlMutex::new(false),
            }
        }
    }

    impl AudioSink for MockAudio {
        fn push_samples(&self, samples: &[f32]) {
            self.samples.lock().extend_from_slice(samples);
        }
        fn sample_rate(&self) -> u32 {
            44_100
        }
    }

    impl HostAudio for MockAudio {
        fn set_volume(&self, volume: f32) {
            *self.volume.lock() = volume;
        }
        fn set_muted(&self, muted: bool) {
            *self.muted.lock() = muted;
        }
    }

    /// Mock frame sink: counts submitted frames.
    struct MockFrames {
        count: AtomicUsize,
    }

    impl MockFrames {
        fn new() -> Self {
            Self {
                count: AtomicUsize::new(0),
            }
        }
    }

    impl FrameSink for MockFrames {
        fn submit(&self, _frame: &[u8; FRAME_BYTES], _frame_index: u64) {
            self.count.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Build a minimal NROM (24 KB). `battery` toggles the battery flag in
    /// the iNES header (flags6 bit 1).
    fn nrom_rom(battery: bool) -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1); // 16 KB PRG
        rom.push(1); // 8 KB CHR
        rom.push(if battery { 0x02 } else { 0x00 });
        rom.push(0x00);
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        // Reset vector → $C000 (16 KB PRG, mirrored).
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0xC0;
        // `JMP $C000` at $C000 (infinite loop). Keeps the test deterministic.
        rom[16] = 0x4C;
        rom[17] = 0x00;
        rom[18] = 0xC0;
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        rom
    }

    #[test]
    fn start_then_stop_persists_battery_save() {
        let dir = tempdir().unwrap();
        let saves_dir = dir.path().join("saves");
        let rom = nrom_rom(true);

        let audio: Arc<dyn HostAudio> = Arc::new(MockAudio::new());
        let frames = Arc::new(MockFrames::new());
        let mut session = EmulatorSession::start_with(&rom, None, saves_dir.clone(), audio, frames)
            .expect("session should start");

        // Poke a couple of buttons (exercises the runtime IPC path).
        session.set_button(SessionPlayer::One, nessie_core::Button::A, true);
        session.set_button(SessionPlayer::Two, nessie_core::Button::Start, true);

        let save_path = session.save_path().unwrap().to_path_buf();
        assert!(save_path.starts_with(&saves_dir));
        assert!(save_path
            .file_name()
            .and_then(|s| s.to_str())
            .map(|s| s.ends_with(".srm"))
            .unwrap_or(false));

        session.stop().unwrap();

        // Save was written and is the size of the cartridge's PRG-RAM.
        assert!(save_path.exists(), "battery save should exist on disk");
        let bytes = fs::read(&save_path).unwrap();
        assert_eq!(bytes.len(), 8 * 1024);
    }

    #[test]
    fn stop_is_idempotent() {
        let dir = tempdir().unwrap();
        let saves_dir = dir.path().join("saves");
        let rom = nrom_rom(true);

        let audio: Arc<dyn HostAudio> = Arc::new(MockAudio::new());
        let frames = Arc::new(MockFrames::new());
        let mut session =
            EmulatorSession::start_with(&rom, None, saves_dir, audio, frames).unwrap();
        session.stop().unwrap();
        session.stop().unwrap(); // second call must be a no-op.
    }

    #[test]
    fn non_battery_cartridge_has_no_save_path() {
        let dir = tempdir().unwrap();
        let saves_dir = dir.path().join("saves");
        let rom = nrom_rom(false);

        let audio: Arc<dyn HostAudio> = Arc::new(MockAudio::new());
        let frames = Arc::new(MockFrames::new());
        let mut session =
            EmulatorSession::start_with(&rom, None, saves_dir.clone(), audio, frames).unwrap();
        assert!(session.save_path().is_none());
        session.stop().unwrap();
        // No .srm file was written.
        if saves_dir.exists() {
            let count = fs::read_dir(&saves_dir).unwrap().count();
            assert_eq!(count, 0, "no save files should be written");
        }
    }

    #[test]
    fn invalid_rom_returns_invalid_rom_error() {
        let dir = tempdir().unwrap();
        let audio: Arc<dyn HostAudio> = Arc::new(MockAudio::new());
        let frames = Arc::new(MockFrames::new());
        let err = EmulatorSession::start_with(b"not a rom", None, dir.path().into(), audio, frames)
            .unwrap_err();
        assert!(matches!(err, AppError::InvalidRom(_)));
    }

    #[test]
    fn unsupported_mapper_returns_typed_error() {
        // Mapper 5 (MMC5) is not supported by the core (FR-2 acceptance
        // bar covers mappers 0/1/2/3/4).
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1);
        rom.push(1);
        rom.push(0x50); // mapper 5 in the high nibble of flags6
        rom.push(0x00);
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        let dir = tempdir().unwrap();
        let audio: Arc<dyn HostAudio> = Arc::new(MockAudio::new());
        let frames = Arc::new(MockFrames::new());
        let err =
            EmulatorSession::start_with(&rom, None, dir.path().into(), audio, frames).unwrap_err();
        assert!(matches!(err, AppError::UnsupportedMapper(5)));
    }

    #[test]
    fn set_volume_and_muted_route_to_audio_sink() {
        let dir = tempdir().unwrap();
        let saves_dir = dir.path().join("saves");
        let rom = nrom_rom(false);

        let mock = Arc::new(MockAudio::new());
        let audio: Arc<dyn HostAudio> = mock.clone();
        let frames = Arc::new(MockFrames::new());
        let mut session =
            EmulatorSession::start_with(&rom, None, saves_dir, audio, frames).unwrap();

        session.set_volume(0.25);
        assert!((*mock.volume.lock() - 0.25).abs() < f32::EPSILON);

        session.set_muted(true);
        assert!(*mock.muted.lock());

        session.set_paused(false);
        assert!(!*mock.muted.lock());

        session.stop().unwrap();
    }

    #[test]
    fn load_save_bytes_returns_none_when_missing() {
        let dir = tempdir().unwrap();
        let bytes = load_save_bytes(dir.path(), "deadbeef").unwrap();
        assert!(bytes.is_none());
    }

    #[test]
    fn load_save_bytes_returns_some_when_present() {
        let dir = tempdir().unwrap();
        let sha1 = "abc123";
        let path = dir.path().join(format!("{sha1}.srm"));
        fs::write(&path, b"hello").unwrap();
        let bytes = load_save_bytes(dir.path(), sha1).unwrap();
        assert_eq!(bytes.as_deref(), Some(b"hello".as_slice()));
    }

    #[test]
    fn battery_save_round_trips_through_start_and_stop() {
        // Pre-seed a battery save, start a session, and confirm the loaded
        // bytes show up in the final snapshot written back on stop.
        let dir = tempdir().unwrap();
        let saves_dir = dir.path().join("saves");
        fs::create_dir_all(&saves_dir).unwrap();
        let rom = nrom_rom(true);

        // 8 KB of a known pattern so we can verify the round-trip.
        let preseed: Vec<u8> = (0..8 * 1024_u32).map(|i| (i & 0xFF) as u8).collect();

        let audio: Arc<dyn HostAudio> = Arc::new(MockAudio::new());
        let frames = Arc::new(MockFrames::new());
        let mut session =
            EmulatorSession::start_with(&rom, Some(preseed.clone()), saves_dir, audio, frames)
                .unwrap();

        let save_path = session.save_path().unwrap().to_path_buf();
        session.stop().unwrap();

        let written = fs::read(&save_path).unwrap();
        // The idle ROM never writes to PRG-RAM, so the pre-loaded bytes
        // come back unchanged.
        assert_eq!(written, preseed);
    }
}
