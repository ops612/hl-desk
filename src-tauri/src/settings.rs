//! App settings shared by the shell (not by one feature): <app data>/settings.json holds
//! "welcome seen", the screenshot capture settings (capture key, mark up after a device
//! screenshot; changed through screenshot.rs commands) and the translate card's last language
//! picks (translate.rs), the offline voice to text switch (voice.rs, Windows) and the AI translation
//! switch (ai.rs). Open at login is kept by the OS itself (tauri-plugin-autostart: a LaunchAgent
//! on macOS, the HKCU Run key on Windows), so it is always read from there, never stored here.
//! Commands are named "settings_<name>" and routed here by lib.rs.

use std::{collections::BTreeMap, path::PathBuf, sync::Mutex};

use serde::{Deserialize, Serialize};
use tauri::{ipc::Invoke, AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::applog;

/// What settings.json holds. Unknown or missing fields fall back to the defaults.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Stored {
    welcome_seen: bool,
    /// Screenshot capture key: "default" (Ctrl/Cmd+Shift+2 only) or "device" (Print Screen on
    /// Windows, Cmd+Shift+4 on macOS, plus Ctrl/Cmd+Shift+2). Used by screenshot/keys.rs.
    capture_key: String,
    /// Offer "Mark up in HL Desk" after a device screenshot (screenshot/watch.rs). On by default
    /// on Windows; off on macOS, where watching the screenshot folder needs Desktop access.
    markup_after_shot: bool,
    /// Translate card: the language pair last picked by hand, per source script ("han",
    /// "latin"), e.g. {"han": "zh-Hans-en"}. Used by translate.rs.
    translate_picks: BTreeMap<String, String>,
    /// Offline voice to text (voice.rs, Windows): the Notes mic records and transcribes in the app
    /// instead of Windows Voice typing. Off by default.
    offline_voice: bool,
    /// AI translation (online, ai.rs): a second result from the company Claude gateway under the
    /// offline one. On by default.
    ai_translate: bool,
}

impl Default for Stored {
    fn default() -> Self {
        Stored {
            welcome_seen: false,
            capture_key: "default".into(),
            markup_after_shot: !cfg!(target_os = "macos"),
            translate_picks: BTreeMap::new(),
            offline_voice: false,
            ai_translate: true,
        }
    }
}

/// Held around every load-modify-save, so two setters cannot undo each other's change.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

fn path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("settings.json"))
}

/// Reads settings.json (missing: the defaults). A file that cannot be parsed is logged; the
/// values that can still be read are kept, and if none can, the file is copied to
/// settings.json.bad before a setter writes over it.
fn load(app: &AppHandle) -> Stored {
    let Some(path) = path(app) else {
        return Stored::default();
    };
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                applog::error(&format!("settings: could not read settings.json: {e}"));
            }
            return Stored::default();
        }
    };
    if let Ok(stored) = serde_json::from_slice::<Stored>(&bytes) {
        return stored;
    }
    applog::error("settings: settings.json could not be parsed");
    if let Some(obj) = serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .as_ref()
        .and_then(|v| v.as_object())
    {
        // Valid JSON with a field of the wrong type: keep every field that still reads.
        let mut stored = Stored::default();
        if let Some(v) = obj.get("welcomeSeen").and_then(|v| v.as_bool()) {
            stored.welcome_seen = v;
        }
        if let Some(v) = obj.get("captureKey").and_then(|v| v.as_str()) {
            stored.capture_key = v.to_string();
        }
        if let Some(v) = obj.get("markupAfterShot").and_then(|v| v.as_bool()) {
            stored.markup_after_shot = v;
        }
        if let Some(v) = obj.get("translatePicks").and_then(|v| v.as_object()) {
            stored.translate_picks = v
                .iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect();
        }
        if let Some(v) = obj.get("offlineVoice").and_then(|v| v.as_bool()) {
            stored.offline_voice = v;
        }
        if let Some(v) = obj.get("aiTranslate").and_then(|v| v.as_bool()) {
            stored.ai_translate = v;
        }
        applog::warn("settings: kept the values that could be read");
        return stored;
    }
    let bad = path.with_file_name("settings.json.bad");
    match std::fs::copy(&path, &bad) {
        Ok(_) => applog::warn(&format!("settings: unreadable settings.json copied to {}", bad.display())),
        Err(e) => applog::error(&format!("settings: could not copy settings.json to {}: {e}", bad.display())),
    }
    Stored::default()
}

fn save(app: &AppHandle, stored: &Stored) -> Result<(), String> {
    let path = path(app).ok_or("no app data folder")?;
    let json = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Load, change, save, under WRITE_LOCK.
fn update(app: &AppHandle, change: impl FnOnce(&mut Stored)) -> Result<(), String> {
    let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut stored = load(app);
    change(&mut stored);
    save(app, &stored)
}

/// Called once at start-up. On the very first run (no settings.json yet) it writes the file and
/// turns on open at login. Returns true on that first run.
pub fn first_run(app: &AppHandle) -> bool {
    {
        let _guard = WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if path(app).is_some_and(|p| p.exists()) {
            return false;
        }
        if let Err(e) = save(app, &Stored::default()) {
            applog::error(&format!("settings: could not write settings.json: {e}"));
        }
    }
    match app.autolaunch().enable() {
        Ok(()) => applog::info("settings: first run, open at login turned on"),
        Err(e) => applog::error(&format!("settings: first run, could not turn on open at login: {e}")),
    }
    true
}

/// The chosen capture key ("default" or "device"); read by screenshot/keys.rs.
pub fn capture_key(app: &AppHandle) -> String {
    load(app).capture_key
}

pub fn set_capture_key(app: &AppHandle, key: &str) -> Result<(), String> {
    update(app, |s| s.capture_key = key.to_string())
}

/// Whether to offer "Mark up in HL Desk" after a device screenshot; read by screenshot/watch.rs.
pub fn markup_after_shot(app: &AppHandle) -> bool {
    load(app).markup_after_shot
}

pub fn set_markup_after_shot(app: &AppHandle, on: bool) -> Result<(), String> {
    update(app, |s| s.markup_after_shot = on)
}

/// The translate card's last manual pick for text in `script`; read by translate.rs.
pub fn translate_pick(app: &AppHandle, script: &str) -> Option<String> {
    load(app).translate_picks.get(script).cloned()
}

pub fn set_translate_pick(app: &AppHandle, script: &str, pair: &str) -> Result<(), String> {
    update(app, |s| {
        s.translate_picks.insert(script.to_string(), pair.to_string());
    })
}

/// Offline voice to text on (Windows); read by voice.rs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn offline_voice(app: &AppHandle) -> bool {
    load(app).offline_voice
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn set_offline_voice(app: &AppHandle, on: bool) -> Result<(), String> {
    update(app, |s| s.offline_voice = on)
}

/// AI translation on (the Settings switch); read by ai.rs.
pub fn ai_translate(app: &AppHandle) -> bool {
    load(app).ai_translate
}

pub fn set_ai_translate(app: &AppHandle, on: bool) -> Result<(), String> {
    update(app, |s| s.ai_translate = on)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct View {
    welcome_seen: bool,
    /// The real state in the OS (LaunchAgent / Run key).
    open_at_login: bool,
}

#[tauri::command]
fn settings_get(app: AppHandle) -> View {
    let open_at_login = app.autolaunch().is_enabled().unwrap_or_else(|e| {
        applog::warn(&format!("settings: could not read open at login: {e}"));
        false
    });
    View { welcome_seen: load(&app).welcome_seen, open_at_login }
}

/// Turns open at login on or off. Returns the state the OS reports afterwards.
#[tauri::command]
fn settings_set_open_at_login(app: AppHandle, on: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let result = if on { manager.enable() } else { manager.disable() };
    if let Err(e) = result {
        applog::error(&format!("settings: open at login {on} failed: {e}"));
        return Err(format!("Could not change open at login: {e}"));
    }
    let now = manager.is_enabled().map_err(|e| e.to_string())?;
    applog::info(&format!("settings: open at login is now {now}"));
    Ok(now)
}

/// "Got it" / "Try a capture" (true), or "Show welcome again" in Settings (false).
#[tauri::command]
fn settings_set_welcome_seen(app: AppHandle, seen: bool) -> Result<(), String> {
    update(&app, |s| s.welcome_seen = seen)
}

pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![settings_get, settings_set_open_at_login, settings_set_welcome_seen]
}
