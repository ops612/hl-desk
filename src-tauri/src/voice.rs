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
    /// Why this PC can't run it (its processor lacks AVX or F16C), or None.
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
            atomic::{AtomicBool, AtomicU32, Ordering},
            mpsc, Arc, Mutex,
        },
        time::{Duration, Instant},
    };

    use cpal::{
        traits::{DeviceTrait, HostTrait, StreamTrait},
        FromSample, Sample, SizedSample,
    };
    use tauri::{ipc::Channel, AppHandle, Emitter, Manager};

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
    /// While recording, the microphone level goes to the note editor's bars this often.
    const LEVEL_EVERY: Duration = Duration::from_millis(100);
    /// Event with the peak (0..1) of the samples recorded since the last one (src/features/notes).
    const LEVEL_EVENT: &str = "voice://level";
    /// whisper.cpp skips input shorter than 1 s; shorter recordings are padded with silence.
    const MIN_PCM: usize = 17_600;

    pub const CPU_REASON: &str = "Not available: this PC's processor lacks AVX or F16C.";
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

    /// whisper.cpp is built for AVX and F16C (build.yml GGML_* and CFLAGS: MSVC /arch:AVX with
    /// __F16C__, SSE up to 4.2; no AVX2, FMA or BMI2 code), so it runs from Ivy Bridge on; on a
    /// processor without them it would crash with an illegal instruction. The check also
    /// covers the OS saving AVX state (is_x86_feature_detected checks OSXSAVE).
    pub fn cpu_supported() -> bool {
        #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
        {
            is_x86_feature_detected!("avx")
                && is_x86_feature_detected!("f16c")
                && is_x86_feature_detected!("sse4.2")
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

    /// One frame (a sample per channel) as one mono sample: their mean. Used by the microphone
    /// callback and by the bench for a stereo WAV.
    fn mono<T>(frame: &[T]) -> f32
    where
        T: Sample,
        f32: FromSample<T>,
    {
        frame.iter().map(|s| f32::from_sample(*s)).sum::<f32>() / frame.len().max(1) as f32
    }

    /// What the audio callback shares with the recording thread. The callback only copies
    /// samples into `buf` (allocated for `max` samples up front, never grown) and raises `peak`;
    /// it never blocks, allocates or logs, as a slow callback makes WASAPI drop audio ("A buffer
    /// underrun or overrun occurred"). The recording thread reads `peak` for the level events
    /// and locks `buf` only after the stream is dropped.
    struct Shared {
        buf: Mutex<Vec<f32>>,
        /// The loudest |sample| since the recording thread last took it, as f32 bits (for
        /// non-negative floats the bit order is the number order, so fetch_max works).
        peak: AtomicU32,
        /// Stream errors (buffer under/overruns); the first one's text, logged when it ends.
        errors: AtomicU32,
        first_error: Mutex<Option<String>>,
    }

    fn build_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        shared: Arc<Shared>,
        max: usize,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = config.channels.max(1) as usize;
        let on_error = shared.clone();
        device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let mut loudest = 0f32;
                // Never waits: the only other user locks it once the stream is gone.
                if let Ok(mut buf) = shared.buf.try_lock() {
                    for frame in data.chunks(channels) {
                        if buf.len() >= max {
                            break;
                        }
                        let s = mono(frame);
                        loudest = loudest.max(s.abs());
                        buf.push(s);
                    }
                }
                if loudest > 0.0 && loudest.is_finite() {
                    shared.peak.fetch_max(loudest.to_bits(), Ordering::Relaxed);
                }
            },
            move |e| {
                // Counted here, logged by the recording thread: no file writes on the audio thread.
                if on_error.errors.fetch_add(1, Ordering::Relaxed) == 0 {
                    if let Ok(mut first) = on_error.first_error.try_lock() {
                        *first = Some(e.to_string());
                    }
                }
            },
            None,
        )
    }

    /// Opens the default microphone and starts it. Returns the stream, what it shares, and its rate.
    fn open_mic() -> Result<(cpal::Stream, Arc<Shared>, u32), String> {
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
        let shared = Arc::new(Shared {
            buf: Mutex::new(Vec::<f32>::with_capacity(max)),
            peak: AtomicU32::new(0),
            errors: AtomicU32::new(0),
            first_error: Mutex::new(None),
        });
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, shared.clone(), max),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, shared.clone(), max),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, config, shared.clone(), max),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, shared.clone(), max),
            other => return Err(format!("The microphone's sample format ({other}) is not supported.")),
        }
        .map_err(|e| mic_error(&e))?;
        stream.play().map_err(|e| mic_error(&e))?;
        Ok((stream, shared, rate))
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
        let app = app.clone();
        std::thread::spawn(move || {
            let (stream, shared, rate) = match open_mic() {
                Ok(opened) => opened,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            // Stop sent, or the Recording dropped (start timed out or was called off): stop now.
            // Meanwhile, every LEVEL_EVERY, send the peak of what came in since the last tick
            // (raised by the audio callback, taken here: this thread is the level timer).
            let deadline = Instant::now() + MAX_RECORDING;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    applog::info("voice: recording reached 120 s and stopped");
                    break;
                }
                match stop_rx.recv_timeout(LEVEL_EVERY.min(left)) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let level = f32::from_bits(shared.peak.swap(0, Ordering::Relaxed)).min(1.0);
                        let _ = app.emit(LEVEL_EVENT, level);
                    }
                    _ => break,
                }
            }
            // Dropping the stream ends its callbacks, so the buffer is free from here on.
            drop(stream);
            let errors = shared.errors.load(Ordering::Relaxed);
            if errors > 0 {
                let first = lock(&shared.first_error).take().unwrap_or_default();
                applog::warn(&format!("voice: microphone stream: {errors} error(s), first: {first}"));
            }
            let samples = std::mem::take(&mut *lock(&shared.buf));
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

    /// Mono samples at their own rate to whisper's input, as for every recording: silence is
    /// refused, the rest goes to 16 kHz and is padded to whisper's 1 s minimum.
    fn prepare(samples: &[f32], rate: u32) -> Result<Vec<f32>, String> {
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        if samples.is_empty() || peak < 1e-4 {
            return Err(SILENT.into());
        }
        let mut pcm = to_16k(samples, rate);
        if pcm.len() < MIN_PCM {
            pcm.resize(MIN_PCM, 0.0);
        }
        Ok(pcm)
    }

    fn write_down(model: &Path, captured: Captured) -> Result<String, String> {
        let Captured { samples, rate } = captured;
        let recording_ms = samples.len() as u64 * 1000 / rate.max(1) as u64;
        let peak = samples.iter().fold(0f32, |m, s| m.max(s.abs()));
        applog::info(&format!("voice: recording {recording_ms} ms at {rate} Hz, peak {peak:.4}"));
        let pcm = prepare(&samples, rate)?;
        let t = Instant::now();
        let ctx = super::load(model)?;
        let load_ms = t.elapsed().as_millis();
        let t = Instant::now();
        let out = super::transcribe_with(&ctx, &pcm, Some("en"), super::thread_count())?;
        let transcribe_ms = t.elapsed().as_millis();
        let text = out.text;
        applog::info(&format!(
            "voice: model load {load_ms} ms, transcribe {transcribe_ms} ms ({}), {} characters out (recording {recording_ms} ms)",
            out.timings,
            text.chars().count()
        ));
        if text.is_empty() {
            return Err("No words were recognised. Try again a little closer to the microphone.".into());
        }
        Ok(text)
    }

    /// The hidden bench (super::bench): `clip` goes through the recording's path (mono, 16 kHz,
    /// padding) and the same transcribe_with as dictation. Returns the "voice bench:" line.
    pub fn bench(model: &Path, clip: &Path, threads: Option<usize>) -> Result<String, String> {
        if !cpu_supported() {
            return Err(CPU_REASON.into());
        }
        let size = std::fs::metadata(model)
            .map_err(|e| format!("model {}: {e}", model.display()))?
            .len();
        let Captured { samples, rate } = read_wav(clip)?;
        let clip_ms = samples.len() as u64 * 1000 / rate.max(1) as u64;
        let pcm = prepare(&samples, rate)?;
        let threads = threads.unwrap_or_else(super::thread_count);
        let t = Instant::now();
        let ctx = super::load(model)?;
        let load_ms = t.elapsed().as_millis();
        let out = super::transcribe_with(&ctx, &pcm, Some("en"), threads)?;
        let tm = &out.timings;
        let name = model.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let text: String = out.text.chars().take(120).collect::<String>().replace('"', "'");
        Ok(format!(
            "voice bench: model {name} {:.1} MB, clip {clip_ms} ms at {rate} Hz, threads {}, audio_ctx {}, load {load_ms} ms, mel {} ms, encode {} ms ({} encoder runs), decode {} ms, steps {}, chars {}, text \"{text}\"",
            size as f64 / 1_048_576.0,
            tm.threads,
            tm.audio_ctx_text(),
            tm.mel_ms,
            tm.encode_ms,
            tm.encoder_runs,
            tm.decode_ms,
            tm.steps,
            out.text.chars().count(),
        ))
    }

    /// A WAV file as mono samples at its own rate: any rate, any channel count (mixed down like
    /// the microphone), 8 to 32-bit integer or 32-bit float.
    fn read_wav(path: &Path) -> Result<Captured, String> {
        let mut reader =
            hound::WavReader::open(path).map_err(|e| format!("clip {}: {e}", path.display()))?;
        let spec = reader.spec();
        let bad = |e: hound::Error| format!("clip {}: {e}", path.display());
        let interleaved: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>().map_err(bad)?,
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample.clamp(1, 32) - 1)) as f32;
                reader
                    .samples::<i32>()
                    .map(|s| s.map(|s| s as f32 * scale))
                    .collect::<Result<_, _>>()
                    .map_err(bad)?
            }
        };
        let samples = interleaved.chunks(spec.channels.max(1) as usize).map(mono).collect();
        Ok(Captured { samples, rate: spec.sample_rate })
    }

    /// Mono samples at `rate` to 16 kHz: each output sample is the mean of the input samples
    /// around it (a box filter as the low-pass). From 48 kHz that is exactly 3 samples per
    /// output sample (tested), which is enough against aliasing for speech.
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

/// whisper threads: the physical cores, at most 4. More (hyper-threads, or every logical core)
/// made whisper.cpp's spinning worker threads wait on each other: 117 s for a 9.5 s clip on a
/// 4-core / 8-thread i7-3630QM. Without a physical count: half the logical cores.
#[cfg_attr(not(windows), allow(dead_code))]
fn threads_for(physical: Option<usize>, logical: usize) -> usize {
    physical.unwrap_or(logical / 2).clamp(1, 4).min(logical.max(1))
}

/// The number of physical processor cores (Windows), or None.
#[cfg(windows)]
fn physical_cores() -> Option<usize> {
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformation, RelationProcessorCore, SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
    };
    let size = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
    let mut len: u32 = 0;
    // SAFETY: a null buffer asks for the needed length in bytes.
    unsafe { GetLogicalProcessorInformation(std::ptr::null_mut(), &mut len) };
    if len == 0 {
        return None;
    }
    let mut buf: Vec<SYSTEM_LOGICAL_PROCESSOR_INFORMATION> = Vec::with_capacity(len as usize / size + 1);
    // SAFETY: buf has room for `len` bytes; the call writes at most that many and sets len.
    let ok = unsafe { GetLogicalProcessorInformation(buf.as_mut_ptr(), &mut len) };
    if ok == 0 {
        return None;
    }
    // SAFETY: the call filled len / size entries.
    unsafe { buf.set_len((len as usize / size).min(buf.capacity())) };
    let cores = buf.iter().filter(|i| i.Relationship == RelationProcessorCore).count();
    (cores > 0).then_some(cores)
}

/// whisper threads for this PC (see threads_for).
#[cfg(windows)]
pub fn thread_count() -> usize {
    let logical = std::thread::available_parallelism().map_or(4, |n| n.get());
    threads_for(physical_cores(), logical)
}

/// The encoder context for a clip of `n_samples` at 16 kHz: whisper's encoder always covers
/// 30 s (1500 frames, 50 per second) unless told less, so a short clip is encoded at its own
/// length plus a margin (at least 256 frames, about 5 s). Clips over 30 s: the full 1500.
#[cfg_attr(not(windows), allow(dead_code))]
fn audio_ctx_for(n_samples: usize) -> i32 {
    const FULL: usize = 1500;
    let ms = n_samples * 1000 / 16_000;
    if ms >= 30_000 {
        return FULL as i32;
    }
    let frames = (ms * FULL).div_ceil(30_000) + 64;
    frames.clamp(256, FULL) as i32
}

/// whisper's window: 30 s at 16 kHz. whisper.cpp itself cuts longer audio into 30 s windows
/// (no timestamps: each window moves on by a full 30 s), runs the full-size encoder on each, and
/// drops a last piece under 1 s; transcribe_with does the same cut itself, so a short last piece
/// gets an encoder sized to it instead of the full 30 s one.
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOW: usize = 30 * 16_000;

/// The pieces whisper runs on: 30 s windows, then the rest; a rest under 1 s is dropped (as
/// whisper.cpp does), except when it is the whole clip.
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_for(n_samples: usize) -> Vec<std::ops::Range<usize>> {
    (0..n_samples)
        .step_by(WINDOW)
        .map(|a| a..(a + WINDOW).min(n_samples))
        .filter(|r| r.start == 0 || r.len() >= 16_000)
        .collect()
}

/// Where the time went in one transcription (for the log line).
#[cfg(windows)]
pub struct Timings {
    pub threads: usize,
    /// The encoder size for each 30 s window (1500 = full).
    pub audio_ctx: Vec<i32>,
    /// Audio to mel spectrogram, until the encoder starts (all windows).
    pub mel_ms: u128,
    /// Encoder runs, each until its first token's logits (includes that first decoder pass).
    pub encode_ms: u128,
    pub encoder_runs: u32,
    /// The rest: decoding.
    pub decode_ms: u128,
    /// Decoder steps (tokens tried; more than the text's tokens means retries).
    pub steps: u32,
}

#[cfg(windows)]
impl Timings {
    /// "1500+1500+256": the encoder size per window.
    pub fn audio_ctx_text(&self) -> String {
        self.audio_ctx.iter().map(|c| c.to_string()).collect::<Vec<_>>().join("+")
    }
}

#[cfg(windows)]
impl std::fmt::Display for Timings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "threads {}, audio_ctx {}, mel {} ms, encode {} ms ({} encoder runs), decode {} ms, {} decoder steps",
            self.threads,
            self.audio_ctx_text(),
            self.mel_ms,
            self.encode_ms,
            self.encoder_runs,
            self.decode_ms,
            self.steps
        )
    }
}

#[cfg(windows)]
pub struct Transcript {
    pub text: String,
    pub timings: Timings,
}

/// Filled by whisper.cpp's callbacks during `full` runs (same thread). Each encoder run is timed
/// from its start to the next logits; whisper.cpp runs the encoder once per 30 s window, so on
/// long audio "encode until the first logits" alone left the later windows' encoder runs in
/// the decode time.
#[cfg(windows)]
struct Probe {
    /// The encoder run still waiting for its first logits.
    encoding: Option<std::time::Instant>,
    /// The first encoder start in the current window (mel time ends there).
    window_encoder: Option<std::time::Instant>,
    encode: std::time::Duration,
    encoder_runs: u32,
    steps: u32,
}

#[cfg(windows)]
unsafe extern "C" fn on_encoder_begin(
    _ctx: *mut whisper_rs::whisper_rs_sys::whisper_context,
    _state: *mut whisper_rs::whisper_rs_sys::whisper_state,
    user_data: *mut std::ffi::c_void,
) -> bool {
    // SAFETY: user_data is the &mut Probe set in transcribe_with, alive for the whole run.
    let probe = unsafe { &mut *(user_data as *mut Probe) };
    let now = std::time::Instant::now();
    probe.encoding = Some(now);
    probe.window_encoder.get_or_insert(now);
    probe.encoder_runs += 1;
    true
}

#[cfg(windows)]
unsafe extern "C" fn on_logits(
    _ctx: *mut whisper_rs::whisper_rs_sys::whisper_context,
    _state: *mut whisper_rs::whisper_rs_sys::whisper_state,
    _tokens: *const whisper_rs::whisper_rs_sys::whisper_token_data,
    _n_tokens: std::os::raw::c_int,
    _logits: *mut f32,
    user_data: *mut std::ffi::c_void,
) {
    // SAFETY: as in on_encoder_begin.
    let probe = unsafe { &mut *(user_data as *mut Probe) };
    if let Some(begin) = probe.encoding.take() {
        probe.encode += begin.elapsed();
    }
    probe.steps += 1;
}

/// Transcribes 16 kHz mono f32 samples with a loaded model on `threads` threads, set up for
/// short dictation: greedy, no temperature retries, no context from earlier text, no
/// timestamps. Each 30 s window (windows_for) is one run, as one segment with an encoder sized
/// to it when it is under 30 s. `language` is a whisper code ("en"); None detects it.
#[cfg(windows)]
pub fn transcribe_with(
    ctx: &WhisperContext,
    pcm_16k_mono_f32: &[f32],
    language: Option<&str>,
    threads: usize,
) -> Result<Transcript, String> {
    let mut state = ctx
        .create_state()
        .map_err(|e| format!("could not start the voice model: {e}"))?;
    let mut probe = Probe {
        encoding: None,
        window_encoder: None,
        encode: std::time::Duration::ZERO,
        encoder_runs: 0,
        steps: 0,
    };
    let mut mel = std::time::Duration::ZERO;
    let mut sizes = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let start = std::time::Instant::now();
    for range in windows_for(pcm_16k_mono_f32.len()) {
        let pcm = &pcm_16k_mono_f32[range];
        let audio_ctx = audio_ctx_for(pcm.len());
        let short = audio_ctx < 1500;
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language.unwrap_or("auto")));
        params.set_n_threads(threads.max(1) as _);
        params.set_temperature(0.0);
        params.set_temperature_inc(0.0);
        params.set_no_context(true);
        params.set_single_segment(short);
        params.set_audio_ctx(if short { audio_ctx } else { 0 });
        params.set_suppress_blank(true);
        params.set_no_timestamps(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        let probe_ptr = &mut probe as *mut Probe as *mut std::ffi::c_void;
        // SAFETY: the callbacks only touch `probe`, which outlives the synchronous `full` call below.
        unsafe {
            params.set_start_encoder_callback(Some(on_encoder_begin));
            params.set_start_encoder_callback_user_data(probe_ptr);
            params.set_filter_logits_callback(Some(on_logits));
            params.set_filter_logits_callback_user_data(probe_ptr);
        }
        let window_start = std::time::Instant::now();
        state
            .full(params, pcm)
            .map_err(|e| format!("voice to text failed: {e}"))?;
        let window_end = std::time::Instant::now();
        // An encoder run with no logits after it (nothing decoded) ends with the window.
        if let Some(begin) = probe.encoding.take() {
            probe.encode += window_end.saturating_duration_since(begin);
        }
        mel += probe
            .window_encoder
            .take()
            .unwrap_or(window_end)
            .saturating_duration_since(window_start);
        sizes.push(if short { audio_ctx } else { 1500 });
        let mut text = String::new();
        for segment in state.as_iter() {
            let part = segment
                .to_str_lossy()
                .map_err(|e| format!("could not read the text: {e}"))?;
            if !is_noise(&part) {
                text.push_str(&part);
            }
        }
        let text = text.trim();
        if !text.is_empty() {
            parts.push(text.to_string());
        }
    }
    let total = start.elapsed();
    Ok(Transcript {
        text: parts.join(" "),
        timings: Timings {
            threads,
            audio_ctx: sizes,
            mel_ms: mel.as_millis(),
            encode_ms: probe.encode.as_millis(),
            encoder_runs: probe.encoder_runs,
            decode_ms: total.saturating_sub(mel).saturating_sub(probe.encode).as_millis(),
            steps: probe.steps,
        },
    })
}

/// The hidden bench's arguments: `--voice-bench <model.bin> <clip.wav> [threads]`.
#[derive(Debug, PartialEq)]
pub(crate) struct BenchArgs {
    pub model: std::path::PathBuf,
    pub clip: std::path::PathBuf,
    /// None: the production default (thread_count).
    pub threads: Option<usize>,
}

/// Finds `--voice-bench` in a command line (first launch, or a second launch's argv); relative
/// paths are taken from `cwd`. None without the flag; Err when its paths are missing.
pub(crate) fn bench_args(argv: &[String], cwd: &std::path::Path) -> Option<Result<BenchArgs, String>> {
    let i = argv.iter().position(|a| a == "--voice-bench")?;
    let rest = &argv[i + 1..];
    let (Some(model), Some(clip)) = (rest.first(), rest.get(1)) else {
        return Some(Err("usage: --voice-bench <model.bin> <clip.wav> [threads]".into()));
    };
    Some(Ok(BenchArgs {
        model: cwd.join(model),
        clip: cwd.join(clip),
        threads: rest.get(2).and_then(|t| t.parse::<usize>().ok()).filter(|t| *t > 0),
    }))
}

/// Runs the bench on its own thread (never the main thread, never the window) and writes its
/// result to the app log as "voice bench: ..." lines, ending with "voice bench: done".
pub(crate) fn bench(args: Result<BenchArgs, String>) {
    use crate::applog;
    let run = move || {
        let result = args.and_then(|a| {
            applog::info(&format!(
                "voice bench: start model {}, clip {}, threads {}",
                a.model.display(),
                a.clip.display(),
                a.threads.map_or("default".to_string(), |t| t.to_string())
            ));
            #[cfg(windows)]
            {
                imp::bench(&a.model, &a.clip, a.threads)
            }
            #[cfg(not(windows))]
            {
                Err::<String, String>("offline voice is only on Windows".into())
            }
        });
        match result {
            Ok(line) => applog::info(&line),
            Err(e) => applog::error(&format!("voice bench: failed: {e}")),
        }
        applog::info("voice bench: done");
    };
    if let Err(e) = std::thread::Builder::new().name("voice-bench".into()).spawn(run) {
        applog::error(&format!("voice bench: could not start its thread: {e}"));
    }
}

/// The recording slot's races and the whisper settings (pure; run on every platform).
#[cfg(test)]
mod slot_tests {
    use super::{audio_ctx_for, bench_args, threads_for, windows_for, BenchArgs, Slot};
    use std::path::{Path, PathBuf};

    #[test]
    fn threads_are_physical_cores_up_to_4() {
        assert_eq!(threads_for(Some(4), 8), 4, "i7-3630QM: 4 cores, 8 threads");
        assert_eq!(threads_for(Some(8), 16), 4);
        assert_eq!(threads_for(Some(2), 4), 2);
        assert_eq!(threads_for(None, 8), 4);
        assert_eq!(threads_for(None, 4), 2);
        assert_eq!(threads_for(None, 1), 1);
        assert_eq!(threads_for(Some(0), 2), 1);
    }

    #[test]
    fn encoder_is_sized_to_the_clip() {
        assert_eq!(audio_ctx_for(16_000), 256, "1 s: the minimum");
        assert_eq!(audio_ctx_for(160_000), 564, "10 s: 500 frames + 64");
        assert_eq!(audio_ctx_for(16_000 * 29), 1500, "29 s: capped");
        assert_eq!(audio_ctx_for(16_000 * 30), 1500);
        assert_eq!(audio_ctx_for(16_000 * 120), 1500);
    }

    #[test]
    fn long_audio_is_cut_into_30_s_windows() {
        let s = 16_000;
        assert_eq!(windows_for(10 * s), vec![0..10 * s]);
        assert_eq!(windows_for(30 * s), vec![0..30 * s]);
        assert_eq!(windows_for(62 * s), vec![0..30 * s, 30 * s..60 * s, 60 * s..62 * s]);
        assert_eq!(windows_for(30 * s + s / 2), vec![0..30 * s], "a last half second is dropped, as whisper.cpp does");
        assert_eq!(windows_for(s / 2), vec![0..s / 2], "a whole short clip is kept");
        assert!(windows_for(0).is_empty());
    }

    #[test]
    fn bench_args_are_read() {
        let argv = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cwd = Path::new("C:/bench");
        assert_eq!(bench_args(&argv(&["hl-desk.exe", "--hidden"]), cwd), None);
        // An absolute clip path (on this OS) is kept as it is.
        let abs: PathBuf = std::env::temp_dir().join("a.wav");
        let abs_arg = abs.to_string_lossy().into_owned();
        assert_eq!(
            bench_args(&argv(&["hl-desk.exe", "--voice-bench", "m.bin", &abs_arg, "2"]), cwd),
            Some(Ok(BenchArgs { model: cwd.join("m.bin"), clip: abs.clone(), threads: Some(2) }))
        );
        assert_eq!(
            bench_args(&argv(&["hl-desk.exe", "--voice-bench", "m.bin", "a.wav"]), cwd),
            Some(Ok(BenchArgs { model: cwd.join("m.bin"), clip: cwd.join("a.wav"), threads: None }))
        );
        assert!(matches!(bench_args(&argv(&["hl-desk.exe", "--voice-bench", "m.bin"]), cwd), Some(Err(_))));
    }

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
        // 48 kHz: each output is the mean of exactly 3 input samples, in order.
        let ramp: Vec<f32> = (0..30).map(|i| i as f32).collect();
        let out = imp::to_16k(&ramp, 48_000);
        assert_eq!(out.len(), 10);
        for (i, s) in out.iter().enumerate() {
            assert!((s - (3 * i + 1) as f32).abs() < 1e-5, "sample {i}: {s}");
        }
    }

    #[test]
    fn physical_core_count_is_sane() {
        let threads = thread_count();
        eprintln!("voice: whisper threads on this runner = {threads}");
        assert!((1..=4).contains(&threads));
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

    /// whisper.cpp must be built for AVX + F16C only (build.yml GGML_* and CFLAGS), or it
    /// crashes on Ivy Bridge PCs (illegal instruction 0xc000001d) or runs about 13x slower
    /// (no F16C); cpu_supported() relies on this.
    #[test]
    fn whisper_is_built_for_avx_and_f16c_only() {
        let info = whisper_rs::print_system_info();
        eprintln!("voice: system {info}");
        assert!(info.contains("AVX = 1"), "AVX should be on: {info}");
        assert!(info.contains("F16C = 1"), "F16C should be on: {info}");
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
        // A ~10 s clip too: the sentence three times with half a second of silence between.
        let mut long = Vec::new();
        for _ in 0..3 {
            long.extend_from_slice(&pcm);
            long.extend(std::iter::repeat(0.0).take(8_000));
        }
        // This PC's thread count, then 2 threads to stand in for a slow PC.
        for (clip, samples) in [("3.6 s", &pcm), ("10 s", &long)] {
            for threads in [thread_count(), 2] {
                let audio_ms = samples.len() as u128 * 1000 / 16_000;
                let t = Instant::now();
                let out = transcribe_with(&ctx, samples, Some("en"), threads).expect("transcribe");
                eprintln!(
                    "voice: RESULT model={} clip={clip} audio_ms={audio_ms} ms={} ({}) text={:?}",
                    imp::MODEL.name,
                    t.elapsed().as_millis(),
                    out.timings,
                    out.text
                );
                assert!(out.text.to_lowercase().contains("supplier"), "unexpected text: {}", out.text);
            }
        }
    }
}
