//! Offline voice to text (Windows only). Opt-in from Settings ("Offline voice to text"): the
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
    /// The Settings switch.
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

/// True when the Settings switch is on (Windows); notes.rs then records here and never sends Win+H.
pub(crate) fn enabled(app: &AppHandle) -> bool {
    #[cfg(windows)]
    {
        crate::settings::offline_voice(app)
    }
    #[cfg(not(windows))]
    {
        let _ = app;
        false
    }
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
        Status { supported: false, enabled: false, installed: false, downloading: false, total_bytes: 0 }
    }
}

#[tauri::command]
fn voice_set_enabled(app: AppHandle, on: bool) -> Result<Status, String> {
    #[cfg(windows)]
    {
        crate::settings::set_offline_voice(&app, on)?;
        crate::applog::info(&format!("voice: offline voice to text {}", if on { "on" } else { "off" }));
        Ok(imp::status(&app))
    }
    #[cfg(not(windows))]
    {
        let _ = (app, on);
        Err("Offline voice to text is only on Windows.".into())
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
        Err("Offline voice to text is only on Windows.".into())
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
        Err("Offline voice to text is only on Windows.".into())
    }
}

/// Stops the recording and throws it away (the note was closed).
#[tauri::command]
fn voice_cancel() {
    #[cfg(windows)]
    imp::cancel();
}

/// Starts recording from the default microphone (notes.rs notes_dictate, when enabled()).
#[cfg(windows)]
pub(crate) fn start(app: &AppHandle) -> Result<(), String> {
    imp::start(app)
}

pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![voice_status, voice_set_enabled, voice_download, voice_stop, voice_cancel]
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

    use super::{Progress, Status};
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

    const NO_MIC: &str = "No microphone found. Plug one in, or turn it on in Windows sound settings.";
    const NO_ACCESS: &str =
        "HL Desk can't use the microphone. Turn on Settings > Privacy > Microphone > Let desktop apps access your microphone.";
    const SILENT: &str =
        "No sound was recorded. Check the microphone, and Settings > Privacy > Microphone > Let desktop apps access your microphone.";
    const MISSING: &str =
        "The voice model isn't downloaded. Turn on Offline voice to text in Settings to download it.";

    static DOWNLOADING: AtomicBool = AtomicBool::new(false);

    fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
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
        Status {
            supported: true,
            enabled: crate::settings::offline_voice(app),
            installed: installed(app),
            downloading: DOWNLOADING.load(Ordering::SeqCst),
            total_bytes: MODEL.size,
        }
    }

    // ---------- download ----------

    pub async fn download(app: &AppHandle, on_progress: Channel<Progress>) -> Result<(), String> {
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

    /// The running recording: its thread owns the cpal stream until `stop` is sent (or
    /// MAX_RECORDING passes), then sends the samples on `done`.
    struct Recording {
        stop: mpsc::Sender<()>,
        done: mpsc::Receiver<Captured>,
    }

    static RECORDING: Mutex<Option<Recording>> = Mutex::new(None);

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

    fn build_stream<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        buf: Arc<Mutex<Vec<f32>>>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = config.channels.max(1) as usize;
        let max = config.sample_rate as usize * (MAX_RECORDING.as_secs() as usize + 1);
        device.build_input_stream::<T, _, _>(
            config,
            move |data: &[T], _| {
                let mut buf = lock(&buf);
                if buf.len() >= max {
                    return;
                }
                for frame in data.chunks(channels) {
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
        let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 10)));
        let format = config.sample_format();
        let config: cpal::StreamConfig = config.into();
        let stream = match format {
            cpal::SampleFormat::F32 => build_stream::<f32>(&device, config, buf.clone()),
            cpal::SampleFormat::I16 => build_stream::<i16>(&device, config, buf.clone()),
            cpal::SampleFormat::I32 => build_stream::<i32>(&device, config, buf.clone()),
            cpal::SampleFormat::U16 => build_stream::<u16>(&device, config, buf.clone()),
            other => return Err(format!("The microphone's sample format ({other}) is not supported.")),
        }
        .map_err(|e| mic_error(&e))?;
        stream.play().map_err(|e| mic_error(&e))?;
        Ok((stream, buf, rate))
    }

    pub fn start(app: &AppHandle) -> Result<(), String> {
        if !installed(app) {
            applog::error("voice: model missing");
            return Err(MISSING.into());
        }
        let mut slot = lock(&RECORDING);
        if slot.is_some() {
            return Err("Already listening.".into());
        }
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
            if let Err(mpsc::RecvTimeoutError::Timeout) = stop_rx.recv_timeout(MAX_RECORDING) {
                applog::info("voice: recording reached 120 s and stopped");
            }
            drop(stream);
            let samples = std::mem::take(&mut *lock(&buf));
            let _ = done_tx.send(Captured { samples, rate });
        });
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {
                *slot = Some(Recording { stop: stop_tx, done: done_rx });
                applog::info("voice: recording");
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err("The microphone did not start.".into()),
        }
    }

    pub fn cancel() {
        if let Some(rec) = lock(&RECORDING).take() {
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
