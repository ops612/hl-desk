//! Translate window (label "translator"): staff type or paste text and get a live offline
//! translation to copy. Front end: translator.html, src/translator/. Capability: translator.json.
//! It uses translate.rs's commands (translate_detect, translate_status, translate_download,
//! translate_model_file, translate_copy, whose own-copy guard keeps the double-copy card away).
//!
//! Opened with Ctrl+Shift+1 (Cmd+Shift+1 on macOS) or the main window's Translate button
//! (translator_open). Created on first use, about 380x440, always on top, centred; Esc or the
//! close button hide it (the page). Opening a hidden window tells the page to start empty;
//! opening a shown one only focuses it.

use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::applog;

const LABEL: &str = "translator";
const OPEN_EVENT: &str = "translator://open";
const WIDTH: f64 = 380.0;
const HEIGHT: f64 = 440.0;

/// Cmd+Shift+1 on macOS, Ctrl+Shift+1 elsewhere.
fn shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let main = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let main = Modifiers::CONTROL;
    Shortcut::new(Some(main | Modifiers::SHIFT), Code::Digit1)
}

/// Opens the window off the main thread: the shortcut handler and sync commands run on it, and
/// building a web view window there can deadlock on Windows (captures and the card do the same).
fn open_soon(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || open(&app));
}

/// Shows the window (creating it on first use) and gives it the keyboard.
fn open(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        let fresh = !window.is_visible().unwrap_or(false);
        let _ = window.unminimize();
        if let Err(e) = window.show() {
            applog::error(&format!("translator: could not show the window: {e}"));
        }
        if let Err(e) = window.set_focus() {
            applog::warn(&format!("translator: could not give the window the keyboard: {e}"));
        }
        let _ = app.emit_to(LABEL, OPEN_EVENT, fresh);
        applog::info(&format!("translator: {}", if fresh { "opened" } else { "focused" }));
        return;
    }
    let builder = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("translator.html".into()))
        .title("HL Desk Translate")
        .inner_size(WIDTH, HEIGHT)
        .min_inner_size(320.0, 360.0)
        .center()
        .decorations(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(true)
        .visible(true);
    // macOS: also over other Spaces and full-screen apps, like the card.
    #[cfg(target_os = "macos")]
    let builder = builder.visible_on_all_workspaces(true);
    match builder.build() {
        Ok(window) => {
            let _ = window.set_focus();
            applog::info("translator: window created");
        }
        Err(e) => applog::error(&format!("translator: could not create the window: {e}")),
    }
}

/// The main window's Translate button.
#[tauri::command]
fn translator_open(app: AppHandle) {
    open_soon(&app);
}

pub fn commands() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![translator_open]
}

/// Called once at start-up: registers the open shortcut. Another app may own it: then the header
/// button still opens the window, and the log says so.
pub fn init(app: &AppHandle) {
    let registered = app.global_shortcut().on_shortcut(shortcut(), |app, _shortcut, event| {
        if event.state() == ShortcutState::Pressed {
            applog::info("translator: shortcut pressed");
            open_soon(app);
        }
    });
    match registered {
        Ok(()) => applog::info("translator: open shortcut registered"),
        Err(e) => applog::warn(&format!(
            "translator: could not register the open shortcut (another app may use it): {e}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcut_is_main_key_shift_1() {
        let s = shortcut();
        #[cfg(target_os = "macos")]
        let main = Modifiers::SUPER;
        #[cfg(not(target_os = "macos"))]
        let main = Modifiers::CONTROL;
        assert_eq!(s.key, Code::Digit1);
        assert_eq!(s.mods, main | Modifiers::SHIFT);
    }
}
