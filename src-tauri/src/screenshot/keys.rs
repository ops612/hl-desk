//! Capture keys. Ctrl+Shift+2 (Cmd+Shift+2 on macOS) is always registered, so there is a key
//! that works whatever the system does. The setting "device" adds the key the computer itself
//! uses for screenshots: Print Screen on Windows, Cmd+Shift+4 on macOS. The system usually owns
//! that key (Snipping Tool, macOS Screenshots), so the Settings sheet shows a hint with a button
//! that opens the exact system setting until the user turns the system's use of it off.

use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::applog;

/// Setting values (settings.json "captureKey").
pub const DEFAULT: &str = "default";
pub const DEVICE: &str = "device";

/// Cmd+Shift+2 on macOS, Ctrl+Shift+2 elsewhere. Always registered.
fn default_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let main = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let main = Modifiers::CONTROL;
    Shortcut::new(Some(main | Modifiers::SHIFT), Code::Digit2)
}

/// The computer's own screenshot key: Cmd+Shift+4 on macOS, Print Screen elsewhere.
fn device_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let shortcut = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::Digit4);
    #[cfg(not(target_os = "macos"))]
    let shortcut = Shortcut::new(None, Code::PrintScreen);
    shortcut
}

#[cfg(target_os = "macos")]
const DEVICE_NAME: &str = "Cmd+Shift+4";
#[cfg(not(target_os = "macos"))]
const DEVICE_NAME: &str = "Print Screen";

fn register(app: &AppHandle, shortcut: Shortcut, name: &'static str) -> Result<(), String> {
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                applog::info(&format!("capture: {name} pressed"));
                super::start_capture(app.clone());
            }
        })
        .map_err(|e| e.to_string())
}

/// Called once at start-up: the default key, then the device key if it is chosen.
pub fn init(app: &AppHandle) {
    match register(app, default_shortcut(), "capture shortcut") {
        Ok(()) => applog::info("screenshot: capture shortcut registered"),
        Err(e) => applog::error(&format!("screenshot: could not register the capture shortcut: {e}")),
    }
    let key = crate::settings::capture_key(app);
    if key == DEVICE {
        apply(app, &key);
    }
}

/// Registers the device key when `key` is "device", unregisters it otherwise. The default key
/// stays registered either way.
pub fn apply(app: &AppHandle, key: &str) {
    let shortcuts = app.global_shortcut();
    let registered = shortcuts.is_registered(device_shortcut());
    if key != DEVICE {
        if registered {
            match shortcuts.unregister(device_shortcut()) {
                Ok(()) => applog::info(&format!("screenshot: {DEVICE_NAME} released")),
                Err(e) => applog::warn(&format!("screenshot: could not release {DEVICE_NAME}: {e}")),
            }
        }
        return;
    }
    if registered {
        return;
    }
    match register(app, device_shortcut(), DEVICE_NAME) {
        Ok(()) => applog::info(&format!("screenshot: {DEVICE_NAME} registered as a capture key")),
        Err(e) => applog::warn(&format!(
            "screenshot: could not register {DEVICE_NAME} (the system or another app may use it): {e}"
        )),
    }
}

/// The device key's state for the Settings sheet: "off" (not chosen), "failed" (could not
/// register it), "owned" (the system still uses it), "check" (could not tell), "ok".
pub fn status(app: &AppHandle) -> &'static str {
    if crate::settings::capture_key(app) != DEVICE {
        return "off";
    }
    if !app.global_shortcut().is_registered(device_shortcut()) {
        return "failed";
    }
    match system_owns_key() {
        Some(true) => "owned",
        Some(false) => "ok",
        None => "check",
    }
}

/// Windows: "Use the Print screen key to open screen capture" (HKCU\Control Panel\Keyboard,
/// PrintScreenKeyForSnippingEnabled = 1). None when the value is not set (Windows 11 may then
/// still use the key for Snipping Tool, so the hint asks the user to check).
#[cfg(windows)]
fn system_owns_key() -> Option<bool> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args(["query", r"HKCU\Control Panel\Keyboard", "/v", "PrintScreenKeyForSnippingEnabled"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None; // value not set
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let value = text
        .lines()
        .find(|l| l.contains("PrintScreenKeyForSnippingEnabled"))?
        .split_whitespace()
        .last()?
        .to_string();
    let owned = value != "0x0";
    applog::info(&format!("screenshot: PrintScreenKeyForSnippingEnabled = {value}"));
    Some(owned)
}

/// macOS: System Settings > Keyboard > Keyboard Shortcuts > Screenshots > "Save picture of
/// selected area as a file" (symbolic hot key 30). Not listed means it is on (the default).
#[cfg(target_os = "macos")]
fn system_owns_key() -> Option<bool> {
    let out = std::process::Command::new("/usr/bin/defaults")
        .args(["export", "com.apple.symbolichotkeys", "-"])
        .output()
        .ok()?;
    if !out.status.success() {
        return Some(true);
    }
    let xml = String::from_utf8_lossy(&out.stdout);
    let Some(start) = xml.find("<key>30</key>") else {
        return Some(true);
    };
    let rest = &xml[start..];
    let Some(enabled) = rest.find("<key>enabled</key>") else {
        return Some(true);
    };
    let after = rest[enabled + "<key>enabled</key>".len()..].trim_start();
    Some(!after.starts_with("<false/>"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_owns_key() -> Option<bool> {
    None
}

/// Opens the system setting that owns the device key.
pub fn open_system_settings() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", "ms-settings:easeofaccess-keyboard"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| e.to_string())?;
        applog::info("settings: opened Settings > Accessibility > Keyboard");
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.Keyboard-Settings.extension")
            .spawn()
            .map_err(|e| e.to_string())?;
        applog::info("settings: opened System Settings > Keyboard");
    }
    Ok(())
}
