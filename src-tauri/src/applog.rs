//! App log: <app data>/hl-desk.log, one line per entry ("2026-11-17T08:00:00.123Z INFO message",
//! UTC). A release build has no console, so anything worth knowing afterwards is written here.
//! When the file grows past 1 MB it is cut to its last ~500 KB.
//! Pages log through the `app_log` command (routed by its "app_" prefix in lib.rs).

use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use tauri::{ipc::Invoke, AppHandle, Manager, WebviewWindow};

const MAX_BYTES: u64 = 1024 * 1024;
const KEEP_BYTES: usize = 500 * 1024;
/// Longest message a page may log, in characters.
const MAX_PAGE_MESSAGE: usize = 2000;

/// The log file; None until init (entries before that go nowhere).
static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Called first in setup. Also logs panics (release builds abort on panic, so this is the only
/// trace a crash leaves).
pub fn init(app: &AppHandle) {
    let path = match app.path().app_data_dir() {
        Ok(dir) => {
            let _ = std::fs::create_dir_all(&dir);
            dir.join("hl-desk.log")
        }
        Err(_) => std::env::temp_dir().join("hl-desk.log"),
    };
    *PATH.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        error(&format!("panic: {info}"));
        default_hook(info);
    }));
}

pub fn info(message: &str) {
    write("INFO", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}

pub fn error(message: &str) {
    write("ERROR", message);
}

fn write(level: &str, message: &str) {
    let line = format!("{} {level} {message}\n", timestamp());
    #[cfg(debug_assertions)]
    eprint!("{line}");
    let guard = PATH.lock().unwrap_or_else(|e| e.into_inner());
    let Some(path) = guard.as_ref() else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
    trim(path);
}

/// Over 1 MB: keep the last ~500 KB, starting at a line boundary.
fn trim(path: &Path) {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_BYTES => {}
        _ => return,
    }
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let cut = bytes.len().saturating_sub(KEEP_BYTES);
    let start = bytes[cut..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(cut, |i| cut + i + 1);
    let _ = std::fs::write(path, &bytes[start..]);
}

/// Now in UTC, ISO 8601 with milliseconds.
fn timestamp() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let secs = ms.div_euclid(1000);
    let (days, day_secs) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        day_secs / 3600,
        day_secs % 3600 / 60,
        day_secs % 60,
        ms.rem_euclid(1000)
    )
}

/// Days since 1970-01-01 to (year, month, day) (Howard Hinnant's civil_from_days).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

// ---------- command ----------

/// A page writes to the log: level "error", "warn" or "info". The window label is added.
#[tauri::command]
fn app_log(window: WebviewWindow, level: String, message: String) {
    let message: String = message.chars().take(MAX_PAGE_MESSAGE).collect();
    let line = format!("[{}] {message}", window.label());
    match level.as_str() {
        "error" => error(&line),
        "warn" => warn(&line),
        _ => info(&line),
    }
}

/// The log's commands. Named "app_<name>" so lib.rs routes them here.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![app_log]
}
