//! App settings shared by the shell (not by one feature): <app data>/settings.json holds
//! "welcome seen". Open at login is kept by the OS itself (tauri-plugin-autostart: a LaunchAgent
//! on macOS, the HKCU Run key on Windows), so it is always read from there, never stored here.
//! Commands are named "settings_<name>" and routed here by lib.rs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::{ipc::Invoke, AppHandle, Manager};
use tauri_plugin_autostart::ManagerExt;

use crate::applog;

/// What settings.json holds. Unknown or missing fields fall back to the defaults.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Stored {
    welcome_seen: bool,
}

fn path(app: &AppHandle) -> Option<PathBuf> {
    let dir = app.path().app_data_dir().ok()?;
    let _ = std::fs::create_dir_all(&dir);
    Some(dir.join("settings.json"))
}

fn load(app: &AppHandle) -> Stored {
    path(app)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(app: &AppHandle, stored: &Stored) -> Result<(), String> {
    let path = path(app).ok_or("no app data folder")?;
    let json = serde_json::to_vec_pretty(stored).map_err(|e| e.to_string())?;
    std::fs::write(path, json).map_err(|e| e.to_string())
}

/// Called once at start-up. On the very first run (no settings.json yet) it writes the file and
/// turns on open at login. Returns true on that first run.
pub fn first_run(app: &AppHandle) -> bool {
    if path(app).is_some_and(|p| p.exists()) {
        return false;
    }
    if let Err(e) = save(app, &Stored::default()) {
        applog::error(&format!("settings: could not write settings.json: {e}"));
    }
    match app.autolaunch().enable() {
        Ok(()) => applog::info("settings: first run, open at login turned on"),
        Err(e) => applog::error(&format!("settings: first run, could not turn on open at login: {e}")),
    }
    true
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
    let mut stored = load(&app);
    stored.welcome_seen = seen;
    save(&app, &stored)
}

pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![settings_get, settings_set_open_at_login, settings_set_welcome_seen]
}
