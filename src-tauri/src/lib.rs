mod applog;
mod clipboard;
mod db;
mod notes;
mod screenshot;

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use tauri::{
    ipc::Invoke,
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent,
};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

/// Cmd+Shift+Space on macOS, Ctrl+Shift+Space elsewhere.
fn toggle_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let main = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let main = Modifiers::CONTROL;
    Shortcut::new(Some(main | Modifiers::SHIFT), Code::Space)
}

/// When the main window last hid itself because it lost focus (Unix ms).
static BLUR_HIDDEN_AT: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn toggle_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
    } else if now_ms().saturating_sub(BLUR_HIDDEN_AT.load(Ordering::SeqCst)) < 500 {
        // A click on the tray icon first takes the focus away (the window hides on blur), then
        // arrives here: that click meant "close", so do not open it again.
    } else {
        show_main_window(app);
    }
}

/// Shows the main window and gives it the keyboard (also used to show a capture error).
pub(crate) fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "windows")]
        place_near_tray(&window);
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Windows: open in the bottom-right corner of the main monitor's work area, just above the
/// taskbar and next to the tray (macOS keeps the centred window from tauri.conf.json).
#[cfg(target_os = "windows")]
fn place_near_tray(window: &tauri::WebviewWindow) {
    let (Ok(Some(monitor)), Ok(size)) = (window.primary_monitor(), window.outer_size()) else {
        return;
    };
    let area = monitor.work_area();
    let margin = (12.0 * monitor.scale_factor()) as i32;
    let x = area.position.x + area.size.width as i32 - size.width as i32 - margin;
    let y = area.position.y + area.size.height as i32 - size.height as i32 - margin;
    let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Each feature has its own command list; commands are named "<feature>_<name>"
    // and routed here by prefix, so a feature never edits this file to add one.
    let clipboard_commands = clipboard::commands();
    let screenshot_commands = screenshot::commands();
    let notes_commands = notes::commands();
    let app_commands = applog::commands();

    tauri::Builder::default()
        // Must be the first plugin. A second launch only wakes this one: it shows the window,
        // or with --capture starts an area capture (lets a script test capture).
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            applog::info(&format!("second launch: {:?}", argv.get(1..).unwrap_or_default()));
            if argv.iter().any(|a| a == "--capture") {
                screenshot::start_capture(app.clone());
            } else {
                show_main_window(app);
            }
        }))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if shortcut == &toggle_shortcut() && event.state() == ShortcutState::Pressed {
                        toggle_window(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_drag::init())
        .invoke_handler(move |invoke: Invoke| {
            let command = invoke.message.command();
            if command.starts_with("app_") {
                app_commands(invoke)
            } else if command.starts_with("clipboard_") {
                clipboard_commands(invoke)
            } else if command.starts_with("screenshot_") {
                screenshot_commands(invoke)
            } else if command.starts_with("notes_") {
                notes_commands(invoke)
            } else {
                false
            }
        })
        .setup(|app| {
            applog::init(app.handle());
            applog::info(&format!(
                "HL Desk {} started (args: {:?})",
                app.package_info().version,
                std::env::args_os().skip(1).collect::<Vec<_>>()
            ));
            // Another app may already own the shortcut: keep running (tray icon still opens
            // HL Desk) and say so in the log, instead of quitting at launch.
            if let Err(e) = app.global_shortcut().register(toggle_shortcut()) {
                applog::warn(&format!(
                    "could not register the open shortcut (another app may use it): {e}"
                ));
            }

            // Windows: a tray popover, so no maximize or minimize button and a fixed size
            // (macOS keeps the window from tauri.conf.json).
            #[cfg(target_os = "windows")]
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_resizable(false);
                let _ = window.set_maximizable(false);
                let _ = window.set_minimizable(false);
            }

            let open = MenuItem::with_id(app, "open", "Open HL Desk", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit HL Desk", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("HL Desk")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_window(tray.app_handle());
                    }
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;

            let handle = app.handle();
            clipboard::init(handle);
            screenshot::init(handle);
            notes::init(handle);

            // A first launch with --capture starts a capture too (same as a second launch).
            if std::env::args_os().any(|a| a == "--capture") {
                screenshot::start_capture(handle.clone());
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            // Other windows (the screenshot editor, the capture overlay) behave normally.
            if window.label() != "main" {
                return;
            }
            match event {
                // Closing the main window only hides it; the app keeps running in the menu bar / tray.
                WindowEvent::CloseRequested { api, .. } => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                // Like a tray popover: a click anywhere else closes it. A capture hides the
                // window before it starts, and the shortcuts sheet is part of this window, so
                // neither is affected.
                WindowEvent::Focused(false) if window.is_visible().unwrap_or(false) => {
                    BLUR_HIDDEN_AT.store(now_ms(), Ordering::SeqCst);
                    let _ = window.hide();
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running HL Desk");
}
