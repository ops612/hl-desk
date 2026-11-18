//! Offline voice to text (Windows only). Opt-in from Settings > Voice to text > Offline mode: the
//! English whisper model (MODEL) is downloaded once; then the Notes mic (notes.rs notes_dictate)
//! records the default microphone with cpal and whisper.cpp (whisper-rs) writes it down here,
//! with no internet. Off, or on macOS, the Notes mic uses the computer's own dictation.
//! Commands are named "voice_<name>" and routed here by lib.rs.

use serde::Serialize;
use tauri::{
    ipc::{Channel, Invoke},
    AppHandle,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    /// This build has offline voice to text (Windows).
    supported: bool,
    /// Why this PC can't run it (its processor lacks AVX2 and co.), or None.
    unsupported_reason: Option<&'static str>,
    /// The Settings switch (false where this PC can't run it).
    enabled: bool,
    /// The model file is on disk.
    installed: bool,
    downloading: bool,
    total_bytes: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: u64,
    total: u64,
}

/// True when the Settings switch is on and this PC can run it; notes.rs then records here and
/// never sends Win+H.
#[cfg(windows)]
pub(crate) fn enabled(app: &AppHandle) -> bool {
    imp::cpu_supported() && crate::settings::offline_voice(app)
}

/// Starts recording from the default microphone (notes.rs notes_dictate, when enabled()).
#[cfg(windows)]
pub(crate) fn start(app: &AppHandle) -> Result<(), String> {
    imp::start(app)
}

#[tauri::command]
fn voice_status(app: AppHandle) -> Status {
    #[cfg(windows)]
    {
        imp::status(&app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Status {
            supported: false,
            unsupported_reason: None,
            enabled: false,
            installed: false,
            downloading: false,
            total_bytes: 0,
        }
    }
}

#[tauri::command]
fn voice_set_enabled(app: AppHandle, on: bool) -> Result<Status, String> {
    #[cfg(windows)]
    {
        if on && !imp::cpu_supported() {
            return Err(imp::CPU_REASON.into());
        }
        crate::settings::set_offline_voice(&app, on)?;
        crate::applog::info(&format!("voice: offline voice to text {}", if on { "on" } else { "off" }));
        Ok(imp::status(&app))
    }
    #[cfg(not(windows))]
    {
        let _ = (app, on);
        Err("Offline mode is only on Windows.".into())
    }
}

/// Downloads the model (progress on `on_progress`). Ok at once when it is already there.
#[tauri::command]
async fn voice_download(app: AppHandle, on_progress: Channel<Progress>) -> Result<(), String> {
    #[cfg(windows)]
    {
        imp::download(&app, on_progress).await
    }
    #[cfg(not(windows))]
    {
        let _ = (app, on_progress);
        Err("Offline mode is only on Windows.".into())
    }
}

/// Stops the recording and returns what was said (whisper runs on a background thread).
#[tauri::command]
async fn voice_stop(app: AppHandle) -> Result<String, String> {
    #[cfg(windows)]
    {
        imp::stop(&app).await
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        Err("Offline mode is only on Windows.".into())
    }
}

/// Stops the recording and throws it away (the note was closed, or Stop came while the
/// microphone was still opening). Async, so it never runs on the main thread.
#[tauri::command]
async fn voice_cancel() {
    #[cfg(windows)]
    imp::cancel();
}

pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![voice_status, voice_set_enabled, voice_download, voice_stop, voice_cancel]
}

/// The recording slot, apart from the audio so its start / stop / cancel races can be tested.
/// A start takes a ticket (`begin`) and only keeps its recording (`opened`) if no stop or cancel
/// came while the microphone was opening; a recording that ended by itself is replaced.
#[cfg_attr(not(windows), allow(dead_code))]
struct Slot<R> {
    ticket: u64,
    starting: Option<u64>,
    rec: Option<R>,
}

#[cfg_attr(not(windows), allow(dead_code))]
impl<R> Slot<R> {
    const fn new() -> Self {
        Slot { ticket: 0, starting: None, rec: None }
    }

    /// A start may begin: nothing starting, and no recording still running (`finished` says
    /// whether the one held ended by itself, for example after 120 s). Returns its ticket.
    fn begin(&mut self, finished: impl FnOnce(&R) -> bool) -> Option<u64> {
        if self.starting.is_some() {
            return None;
        }
        if let Some(rec) = &self.rec {
            if !finished(rec) {
                return None;
            }
            self.rec = None;
        }
        self.ticket += 1;
        self.starting = Some(self.ticket);
        Some(self.ticket)
    }

    /// The microphone opened: keeps `rec`, or gives it back (to be stopped) when a stop or
    /// cancel came meanwhile.
    fn opened(&mut self, ticket: u64, rec: R) -> Result<(), R> {
        if self.starting == Some(ticket) {
            self.starting = None;
            self.rec = Some(rec);
            Ok(())
        } else {
            Err(rec)
        }
    }

    /// The start failed or timed out.
    fn failed(&mut self, ticket: u64) {
        if self.starting == Some(ticket) {
            self.starting = None;
        }
    }

    /// Stop or cancel: takes the running recording; a start still opening is called off.
    fn take(&mut self) -> Option<R> {
        self.starting = None;
        self.rec.take()
    }
}

#[cfg(windows)]
mod imp {
    use std::{
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc, Arc, Mutex,
        },
        time::{Duration, Instant},
    };

    use cpal::{
        traits::{DeviceTrait, HostTrait, StreamTrait},
        FromSample, Sample, SizedSample,
    };
    use tauri::{ipc::Channel, AppHandle, Manager};

    use super::{Progress, Slot, Status};
    use crate::{applog, translate::ModelFile};

    /// The model, pinned to one Hugging Face commit of ggerganov/whisper.cpp (licence MIT):
    /// English-only base. Swap the host here; the size and sha256 must stay the file's.
    pub const MODEL_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/ggml-base.en.bin";
    pub const MODEL: ModelFile = ModelFile {
        kind: "whisper",
        name: "ggml-base.en.bin",
        size: 147_964_211,
        sha256: "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002",
    };

    /// Recording stops by itself after this long.
    const MAX_RECORDING: Duration = Duration::from_secs(120);
    /// whisper.cpp skips input shorter than 1 s; shorter recordings are padded with silence.
    const MIN_PCM: usize = 17_600;

    pub const CPU_REASON: &str = "Not available: this PC's processor lacks AVX.";
    const NO_MIC: &str = "No microphone found. Plug one in, or turn it on in Windows sound settings.";
    const NO_ACCESS: &str =
        "HL Desk can't use the microphone. Turn on Settings > Privacy > Microphone > Let desktop apps access your microphone.";
    const SILENT: &str =
        "No sound was recorded. Check the microphone, and Settings > Privacy > Microphone > Let desktop apps access your microphone.";
    const MISSING: &str =
        "The voice model isn't downloaded. Turn on Offline mode (Settings > Voice to text) to download it.";
    const STILL_DOWNLOADING: &str = "Offline mode is still downloading (Settings > Voice to text).";

    static DOWNLOADING: AtomicBool = AtomicBool::new(false);

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// whisper.cpp is built for AVX only (build.yml GGML_* flags: MSVC /arch:AVX, with SSE up to
    /// 4.2; no AVX2, FMA, BMI2 or F16C code), so it runs from Sandy Bridge on; on a processor
    /// without AVX it would crash with an illegal instruction. The check also covers the OS
    /// saving AVX state (is_x86_feature_detected checks OSXSAVE).
    pub fn cpu_supported() -> bool {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            is_x86_feature_detected!("avx") && is_x86_feature_detected!("sse4.2")
        }
        #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
        {
            true
        }
    }

    fn model_path(app: &AppHandle) -> Result<PathBuf, String> {
        let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
        Ok(dir.join("models").join("whisper").join(MODEL.name))
    }

    /// The file is there with its pinned size (its sha256 was checked when it was downloaded).
    fn installed(app: &AppHandle) -> bool {
        model_path(app).is_ok_and(|p| std::fs::metadata(p).is_ok_and(|m| m.len() == MODEL.size))
    }

    pub fn status(app: &AppHandle) -> Status {
        let cpu = cpu_supported();
        Status {
            supported: true,
            unsupported_reason: (!cpu).then_some(CPU_REASON),
            enabled: cpu && crate::settings::offline_voice(app),
            installed: installed(app),
            downloading: DOWNLOADING.load(Ordering::SeqCst),
            total_bytes: MODEL.size,
        }
    }

    // ---------- download ----------

    pub async fn download(app: &AppHandle, on_progress: Channel<Progress>) -> Result<(), String> {
        if !cpu_supported() {
            return Err(CPU_REASON.into());
        }
        if installed(app) {
            let _ = on_progress.send(Progress { done: MODEL.size, total: MODEL.size });
            return Ok(());
        }
        if DOWNLOADING.swap(true, Ordering::SeqCst) {
            return Err("The voice model is already downloading.".into());
        }
        applog::info(&format!("voice: downloading {} ({} bytes) from {MODEL_URL}", MODEL.name, MODEL.size));
        let result = async {
            let target = model_path(app)?;
            let dir = target.parent().ok_or("no model folder")?;
            std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
            let client = crate::translate::http_client()?;
            let report = |done: u64| {
                let _ = on_progress.send(Progress { done, total: MODEL.size });
            };
            crate::translate::download_file(&client, dir, MODEL_URL, &MODEL, &report).await
        }
        .await;
        DOWNLOADING.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => {
                applog::info("voice: model downloaded");
                Ok(())
            }
            Err(e) => {
                applog::error(&format!("voice: model download failed: {e}"));
                Err(format!("Download failed: {e}"))
            }
        }
    }

    // ---------- recording ----------

    /// Mono samples at the microphone's own rate.
    struct Captured {
        samples: Vec<f32>,
        rate: u32,
    }

    /// A recording: its thread owns the cpal stream until `stop` is sent or dropped (or
    /// MAX_RECORDING passes), then sends the samples on `done`.
    struct Recording {
        stop: mpsc::Sender<()>,
        done: mpsc::Receiver<Captured>,
    }

    static RECORDING: Mutex<Slot<Recording>> = Mutex::new(Slot::new());

    /// Its thread has ended (sent its samples or gone): the recording stopped by itself.
    fn finished(rec: &Recording) -> bool {
        !matches!(rec.done.try_recv(), Err(mpsc::TryRecvError::Empty))
    }

    /// Turns a cpal error into a message for the user (access denied, no device, or the error).
    fn mic_error(e: &cpal::Error) -> String {
        let text = e.to_string();
        let lower = text.to_lowercase();
        applog::error(&format!("voice: microphone error ({:?}): {text}", e.kind()));
        if e.kind() == cpal::ErrorKind::PermissionDenied
            || lower.contains("0x80070005")
            || lower.contains("access is denied")
            || lower.contains("access denied")
        {
            NO_ACCESS.into()
        } else if e.kind() == cpal::ErrorKind::DeviceNotAvailable {
            NO_MIC.into()
        } else {
            format!("Could not start the microphone: {text}")
        }
    }

    /// The input stream; each callback downmixes to mono into `buf`, which was allocated for
    /// `max` samples up front and is never grown.
    fn build_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        buf: Arc<Mutex<Vec<f32>>>,
        max: usize,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = config.channels.max(1) as usize;
        device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let mut buf = lock(&buf);
                for frame in data.chunks(channels) {
                    if buf.len() >= max {
                        break;
                    }
                    let sum: f32 = frame.iter().map(|s| f32::from_sample(*s)).sum();
                    buf.push(sum / frame.len() as f32);
                }
            },
            |e| applog::warn(&format!("voice: microphone stream: {e}")),
            None,
        )
    }

    /// Opens the default microphone and starts it. Returns the stream, its buffer and rate.
    fn open_mic() -> Result<(cpal::Stream, Arc<Mutex<Vec<f32>>>, u32), String> {
        let host = cpal::default_host();
        let device = host.default_input_device().ok_or_else(|| {
            applog::error("voice: no default input device");
            NO_MIC.to_string()
        })?;
        let config = device.default_input_config().map_err(|e| mic_error(&e))?;
        let rate = config.sample_rate();
        applog::info(&format!(
            "voice: microphone {} Hz, {} channel(s), {:?}",
            rate,
            config.channels(),
            config.sample_format()
        ));
        let max = rate as usize * (MAX_RECORDING.as_secs() as usize + 1);
        let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(max)));
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, buf.clone(), max),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, buf.clone(), max),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, config, buf.clone(), max),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, buf.clone(), max),
            other => return Err(format!("The microphone's sample format ({other}) is not supported.")),
        }
        .map_err(|e| mic_error(&e))?;
        stream.play().map_err(|e| mic_error(&e))?;
        Ok((stream, buf, rate))
    }

    pub fn start(app: &AppHandle) -> Result<(), String> {
        if !cpu_supported() {
            return Err(CPU_REASON.into());
        }
        if !installed(app) {
            let downloading = DOWNLOADING.load(Ordering::SeqCst);
            applog::error(&format!("voice: model missing (downloading: {downloading})"));
            return Err(if downloading { STILL_DOWNLOADING } else { MISSING }.into());
        }
        // The lock is held only for these quick steps, never while the microphone opens.
        let ticket = lock(&RECORDING).begin(finished).ok_or("Already listening.")?;
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (done_tx, done_rx) = mpsc::channel::<Captured>();
        std::thread::spawn(move || {
            let (stream, buf, rate) = match open_mic() {
                Ok(opened) => opened,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            // Stop sent, or the Recording dropped (start timed out or was called off): stop now.
            if let Err(mpsc::RecvTimeoutError::Timeout) = stop_rx.recv_timeout(MAX_RECORDING) {
                applog::info("voice: recording reached 120 s and stopped");
            }
            drop(stream);
            let samples = std::mem::take(&mut *lock(&buf));
            let _ = done_tx.send(Captured { samples, rate });
        });
        let rec = Recording { stop: stop_tx, done: done_rx };
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => match lock(&RECORDING).opened(ticket, rec) {
                Ok(()) => {
                    applog::info("voice: recording");
                    Ok(())
                }
                Err(rec) => {
                    let _ = rec.stop.send(());
                    applog::info("voice: stopped while the microphone was opening");
                    Err("Stopped.".into())
                }
            },
            Ok(Err(e)) => {
                lock(&RECORDING).failed(ticket);
                Err(e)
            }
            Err(_) => {
                lock(&RECORDING).failed(ticket);
                let _ = rec.stop.send(());
                applog::error("voice: the microphone did not open within 10 s");
                Err("The microphone did not start.".into())
            }
        }
    }

    pub fn cancel() {
        let rec = lock(&RECORDING).take();
        if let Some(rec) = rec {
            let _ = rec.stop.send(());
            applog::info("voice: recording cancelled");
        }
    }

    pub async fn stop(app: &AppHandle) -> Result<String, String> {
        let rec = lock(&RECORDING).take().ok_or("Not listening.")?;
        let _ = rec.stop.send(());
        let path = model_path(app)?;
        tauri::async_runtime::spawn_blocking(move || {
            let captured = rec
                .done
                .recv_timeout(Duration::from_secs(10))
                .map_err(|_| "The recording was lost.".to_string())?;
            write_down(&path, captured)
        })
        .await
        .map_err(|e| e.to_string())?
    }

    fn write_down(model: &Path, captured: Captured) -> Result<String, String> {
        let Captured { samples, rate } = captured;
        let recording_ms = samples.len() as u64 * 1000 / rate.max(1) as u64;
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        applog::info(&format!("voice: recording {recording_ms} ms at {rate} Hz, peak {peak:.4}"));
        if samples.is_empty() || peak < 1e-4 {
            return Err(SILENT.into());
        }
        let mut pcm = to_16k(&samples, rate);
        if pcm.len() < MIN_PCM {
            pcm.resize(MIN_PCM, 0.0);
        }
        let t = Instant::now();
        let ctx = super::load(model)?;
        let load_ms = t.elapsed().as_millis();
        let t = Instant::now();
        let (text, _) = super::transcribe_with(&ctx, &pcm, Some("en"))?;
        let transcribe_ms = t.elapsed().as_millis();
        applog::info(&format!(
            "voice: model load {load_ms} ms, transcribe {transcribe_ms} ms, {} characters out (recording {recording_ms} ms)",
            text.chars().count()
        ));
        if text.is_empty() {
            return Err("No words were recognised. Try again a little closer to the microphone.".into());
        }
        Ok(text)
    }

    /// Mono samples at `rate` to 16 kHz: each output sample is the mean of the input samples
    /// around it (a box filter, enough against aliasing for speech).
    pub fn to_16k(input: &[f32], rate: u32) -> Vec<f32> {
        if rate == 16_000 || input.is_empty() {
            return input.to_vec();
        }
        let ratio = rate as f64 / 16_000.0;
        let n = (input.len() as f64 / ratio).floor() as usize;
        let half = (ratio / 2.0).max(0.5);
        (0..n)
            .map(|i| {
                let center = (i as f64 + 0.5) * ratio;
                let lo = ((center - half).floor().max(0.0) as usize).min(input.len() - 1);
                let hi = ((center + half).ceil() as usize).clamp(lo + 1, input.len());
                input[lo..hi].iter().sum::<f32>() / (hi - lo) as f32
            })
            .collect()
    }
}

#[cfg(windows)]
use std::path::Path;

#[cfg(windows)]
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// Loads a ggml whisper model.
#[cfg(windows)]
pub fn load(model_path: &Path) -> Result<WhisperContext, String> {
    let path = model_path.to_str().ok_or("model path is not valid UTF-8")?;
    WhisperContext::new_with_params(path, WhisperContextParameters::default())
        .map_err(|e| format!("could not load the voice model: {e}"))
}

/// whisper's markers for non-speech ("[BLANK_AUDIO]", "(music)"), dropped from the text.
#[cfg(windows)]
fn is_noise(segment: &str) -> bool {
    let s = segment.trim();
    s.is_empty() || (s.starts_with('[') && s.ends_with(']')) || (s.starts_with('(') && s.ends_with(')'))
}

/// Transcribes 16 kHz mono f32 samples with a loaded model. `language` is a whisper code
/// ("en"); None detects it. Returns the text and the language used.
#[cfg(windows)]
pub fn transcribe_with(
    ctx: &WhisperContext,
    pcm_16k_mono_f32: &[f32],
    language: Option<&str>,
) -> Result<(String, String), String> {
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("could not start the voice model: {e}"))?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_language(Some(language.unwrap_or("auto")));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    params.set_n_threads(threads as _);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state
        .full(params, pcm_16k_mono_f32)
        .map_err(|e| format!("voice to text failed: {e}"))?;
    let mut text = String::new();
    for segment in state.as_iter() {
        let part = segment
            .to_str_lossy()
            .map_err(|e| format!("could not read the text: {e}"))?;
        if !is_noise(&part) {
            text.push_str(&part);
        }
    }
    let lang = whisper_rs::get_lang_str(state.full_lang_id_from_state())
        .unwrap_or("?")
        .to_string();
    Ok((text.trim().to_string(), lang))
}

/// The recording slot's races (pure; runs on every platform).
#[cfg(test)]
mod slot_tests {
    use super::Slot;

    #[test]
    fn one_start_at_a_time() {
        let mut slot: Slot<&str> = Slot::new();
        let t = slot.begin(|_| false).expect("first start");
        assert!(slot.begin(|_| false).is_none(), "a second start while opening");
        assert!(slot.opened(t, "rec").is_ok());
        assert!(slot.begin(|_| false).is_none(), "a start while recording");
        assert_eq!(slot.take(), Some("rec"));
        assert!(slot.begin(|_| false).is_some(), "a start after stop");
    }

    #[test]
    fn stop_while_opening_calls_the_start_off() {
        let mut slot: Slot<&str> = Slot::new();
        let t = slot.begin(|_| false).unwrap();
        assert_eq!(slot.take(), None, "nothing recorded yet");
        assert_eq!(slot.opened(t, "rec"), Err("rec"), "the opened recording is handed back to stop");
        assert!(slot.begin(|_| false).is_some(), "not stuck");
    }

    #[test]
    fn a_stale_start_cannot_take_a_newer_slot() {
        let mut slot: Slot<&str> = Slot::new();
        let old = slot.begin(|_| false).unwrap();
        slot.take();
        let new = slot.begin(|_| false).unwrap();
        assert_eq!(slot.opened(old, "old"), Err("old"));
        assert!(slot.opened(new, "new").is_ok());
    }

    #[test]
    fn failed_start_frees_the_slot() {
        let mut slot: Slot<&str> = Slot::new();
        let t = slot.begin(|_| false).unwrap();
        slot.failed(t);
        assert!(slot.begin(|_| false).is_some());
    }

    #[test]
    fn a_recording_that_ended_by_itself_is_replaced() {
        let mut slot: Slot<&str> = Slot::new();
        let t = slot.begin(|_| false).unwrap();
        slot.opened(t, "rec").unwrap();
        assert!(slot.begin(|_| true).is_some());
        assert_eq!(slot.take(), None, "the ended recording was dropped");
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    #[test]
    fn model_manifest_is_pinned() {
        assert!(imp::MODEL_URL.contains("/resolve/5359861c739e955e79d9a303bcbc70fb988958b1/"));
        assert!(imp::MODEL_URL.ends_with(imp::MODEL.name));
        assert_eq!(imp::MODEL.sha256.len(), 64);
    }

    #[test]
    fn resamples_48k_to_16k() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()).collect();
        let out = imp::to_16k(&input, 48_000);
        assert_eq!(out.len(), 16_000);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        let flat = imp::to_16k(&[0.5; 44_100], 44_100);
        assert_eq!(flat.len(), 16_000);
        assert!(flat.iter().all(|s| (s - 0.5).abs() < 1e-6));
        assert_eq!(imp::to_16k(&[0.1, 0.2], 16_000), vec![0.1, 0.2]);
    }

    #[test]
    fn drops_noise_markers() {
        assert!(is_noise(" [BLANK_AUDIO]"));
        assert!(is_noise("(music)"));
        assert!(!is_noise(" Call the supplier."));
    }

    #[test]
    fn cpu_check_runs() {
        // CI's runner has AVX; the check itself must not panic.
        assert!(imp::cpu_supported());
    }

    /// whisper.cpp must be built for AVX only (build.yml GGML_* flags), or it crashes on
    /// Ivy Bridge PCs (illegal instruction 0xc000001d); cpu_supported() relies on this.
    #[test]
    fn whisper_is_built_for_avx_only() {
        let info = whisper_rs::print_system_info();
        eprintln!("voice: system {info}");
        assert!(info.contains("AVX = 1"), "AVX should be on: {info}");
        for flag in ["AVX2 = 1", "FMA = 1", "BMI2 = 1", "AVX512 = 1"] {
            assert!(!info.contains(flag), "{flag} in a build that must run without it: {info}");
        }
    }

    fn read_wav(path: &Path) -> Vec<f32> {
        let mut reader = hound::WavReader::open(path).expect("open wav");
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 16_000, "{} must be 16 kHz", path.display());
        assert_eq!(spec.channels, 1, "{} must be mono", path.display());
        assert_eq!(spec.bits_per_sample, 16, "{} must be 16-bit", path.display());
        reader
            .samples::<i16>()
            .map(|s| s.expect("wav sample") as f32 / 32768.0)
            .collect()
    }

    /// Fixture voice-en.wav: made once on the macos-latest CI runner with `say -v Samantha`
    /// "Please remind me to call the supplier about the delivery on Friday.", then
    /// `afconvert -f WAVE -d LEI16@16000 -c 1` (16 kHz mono 16-bit, 3.6 s).
    ///
    /// Runs only where HL_WHISPER_MODELS points at a folder with ggml-base.en.bin (the Windows
    /// CI build downloads it); elsewhere it prints a skip line.
    #[test]
    fn transcribes_english_fixture() {
        let Some(dir) = std::env::var_os("HL_WHISPER_MODELS").map(PathBuf::from) else {
            eprintln!("voice: HL_WHISPER_MODELS not set, skipping");
            return;
        };
        let model = dir.join(imp::MODEL.name);
        assert!(model.exists(), "HL_WHISPER_MODELS is set but {} is missing", model.display());
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/voice-en.wav");
        eprintln!("voice: whisper.cpp {}", whisper_rs::get_whisper_version());
        eprintln!("voice: system {}", whisper_rs::print_system_info());
        let t = Instant::now();
        let ctx = load(&model).expect("load model");
        eprintln!("voice: {} loaded in {} ms", imp::MODEL.name, t.elapsed().as_millis());
        let pcm = read_wav(&fixture);
        let audio_ms = pcm.len() as u128 * 1000 / 16_000;
        let t = Instant::now();
        let (text, _) = transcribe_with(&ctx, &pcm, Some("en")).expect("transcribe");
        eprintln!(
            "voice: RESULT model={} audio_ms={audio_ms} ms={} text={text:?}",
            imp::MODEL.name,
            t.elapsed().as_millis()
        );
        assert!(text.to_lowercase().contains("supplier"), "unexpected text: {text}");
    }
}
