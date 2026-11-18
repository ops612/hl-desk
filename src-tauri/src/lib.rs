mod applog;
mod clipboard;
mod db;
mod notes;
mod ocr;
mod screenshot;
mod settings;
mod translate;
mod voice;

use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
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
/// When the main window was last shown (Unix ms). A focus loss right after showing is ignored:
/// Windows may refuse the focus to a window opened at start-up (first run, welcome card), and
/// hiding it then would mean the user never sees it.
static SHOWN_AT: AtomicU64 = AtomicU64::new(0);
/// The header's pin button: while pinned the main window does not hide on blur (Esc and the
/// tray icon still hide it). It is always on top either way (tauri.conf.json alwaysOnTop).
/// main.ts remembers the choice and sets it at start.
static PINNED: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn shell_set_pinned(pinned: bool) {
    PINNED.store(pinned, Ordering::SeqCst);
}

/// The page found it was just updated (src/updater.ts): show the window even if this start had
/// --hidden (the updater restarts HL Desk with the arguments it had).
#[tauri::command]
fn shell_show_main(app: AppHandle) {
    applog::info("showing the window after an update");
    show_main_window(&app);
}

/// Links the pages may open in the default browser or System Settings. Nothing else is opened.
const OPEN_URL_PREFIXES: [&str; 2] = [
    "https://github.com/ops612/hl-desk-releases/",
    "x-apple.systempreferences:",
];

#[tauri::command]
fn shell_open_url(url: String) -> Result<(), String> {
    if !OPEN_URL_PREFIXES.iter().any(|p| url.starts_with(p)) || url.contains(['"', ' ', '\n']) {
        return Err("This link is not allowed.".into());
    }
    applog::info(&format!("open url: {url}"));
    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(target_os = "windows")]
    let opened = std::process::Command::new("explorer.exe").arg(&url).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opened = std::process::Command::new("xdg-open").arg(&url).spawn();
    opened.map(|_| ()).map_err(|e| {
        applog::error(&format!("open url failed: {e}"));
        e.to_string()
    })
}

/// Settings > Open-source notices: opens THIRD_PARTY_NOTICES.md (bundled as a resource,
/// tauri.conf.json) in a text editor (TextEdit on macOS, Notepad on Windows).
#[tauri::command]
fn shell_open_notices(app: AppHandle) -> Result<(), String> {
    let path = app
        .path()
        .resource_dir()
        .map_err(|e| e.to_string())?
        .join("THIRD_PARTY_NOTICES.md");
    if !path.exists() {
        applog::error(&format!("open notices: {} is missing", path.display()));
        return Err("The notices file is missing.".into());
    }
    applog::info(&format!("open notices: {}", path.display()));
    #[cfg(target_os = "macos")]
    let opened = std::process::Command::new("open").arg("-t").arg(&path).spawn();
    #[cfg(target_os = "windows")]
    let opened = std::process::Command::new("notepad.exe").arg(&path).spawn();
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let opened = std::process::Command::new("xdg-open").arg(&path).spawn();
    opened.map(|_| ()).map_err(|e| {
        applog::error(&format!("open notices failed: {e}"));
        e.to_string()
    })
}

fn shell_handlers() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![shell_set_pinned, shell_show_main, shell_open_url, shell_open_notices]
}

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
        SHOWN_AT.store(now_ms(), Ordering::SeqCst);
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

/// The tray / menu-bar icon (src-tauri/icons/tray/, made by scripts/make-tray-icons.mjs).
/// macOS: an 18pt template image (36 px for Retina). Windows: the coloured icon in the size that
/// matches the main monitor's scale (16 px at 100%, 24 px at 150%, 32 px at 200%).
fn tray_icon(app: &tauri::App) -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        tauri::include_image!("icons/tray/tray-mac@2x.png")
    }
    #[cfg(not(target_os = "macos"))]
    {
        let scale = app
            .handle()
            .primary_monitor()
            .ok()
            .flatten()
            .map_or(1.0, |m| m.scale_factor());
        let px = 16.0 * scale;
        if px <= 16.5 {
            tauri::include_image!("icons/tray/tray-16.png")
        } else if px <= 20.5 {
            tauri::include_image!("icons/tray/tray-20.png")
        } else if px <= 24.5 {
            tauri::include_image!("icons/tray/tray-24.png")
        } else if px <= 32.5 {
            tauri::include_image!("icons/tray/tray-32.png")
        } else if px <= 40.5 {
            tauri::include_image!("icons/tray/tray-40.png")
        } else {
            tauri::include_image!("icons/tray/tray-48.png")
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Each feature has its own command list; commands are named "<feature>_<name>"
    // and routed here by prefix, so a feature never edits this file to add one.
    let clipboard_commands = clipboard::commands();
    let screenshot_commands = screenshot::commands();
    let notes_commands = notes::commands();
    let ocr_commands = ocr::commands();
    let settings_commands = settings::commands();
    let translate_commands = translate::commands();
    let voice_commands = voice::commands();
    let app_commands = applog::commands();
    let shell_commands = shell_handlers();

    tauri::Builder::default()
        // Must be the first plugin. A second launch only wakes this one: it shows the window,
        // or with --capture starts an area capture (lets a script test capture), or with
        // --voice-bench <model.bin> <clip.wav> [threads] runs the hidden voice bench (app log).
        .plugin(tauri_plugin_single_instance::init(|app, argv, cwd| {
            applog::info(&format!("second launch: {:?}", argv.get(1..).unwrap_or_default()));
            if let Some(bench) = voice::bench_args(&argv, std::path::Path::new(&cwd)) {
                voice::bench(bench);
            } else if argv.iter().any(|a| a == "--capture") {
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
        // Open at login: a LaunchAgent on macOS (the plugin's default), the HKCU Run key on
        // Windows, both named "HL Desk". Started at login it gets --hidden: tray icon only.
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("HL Desk")
                .arg("--hidden")
                .build(),
        )
        // Auto-update (tauri.conf.json plugins.updater): src/updater.ts checks and downloads in
        // the background and installs only on the user's click. Windows: install() runs the NSIS
        // installer (passive) and exits; the installer starts HL Desk again. macOS: the page
        // relaunches through the process plugin.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
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
            } else if command.starts_with("ocr_") {
                ocr_commands(invoke)
            } else if command.starts_with("settings_") {
                settings_commands(invoke)
            } else if command.starts_with("translate_") {
                translate_commands(invoke)
            } else if command.starts_with("voice_") {
                voice_commands(invoke)
            } else if command.starts_with("shell_") {
                shell_commands(invoke)
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
            tray = tray.icon(tray_icon(app));
            #[cfg(target_os = "macos")]
            {
                // Black + alpha: macOS draws it in the menu bar's own colour (light or dark).
                tray = tray.icon_as_template(true);
            }
            tray.build(app)?;

            let handle = app.handle();
            clipboard::init(handle);
            screenshot::init(handle);
            notes::init(handle);
            translate::init(handle);

            // First run: open at login is turned on and the window opens with the welcome card.
            // Started at login (--hidden): tray icon only. Started by hand: show the window.
            let first_run = settings::first_run(handle);
            let has_arg = |name: &str| std::env::args_os().any(|a| a == name);
            let args: Vec<String> =
                std::env::args_os().map(|a| a.to_string_lossy().into_owned()).collect();
            let cwd = std::env::current_dir().unwrap_or_default();
            if let Some(bench) = voice::bench_args(&args, &cwd) {
                // A first launch with --voice-bench runs the bench too, with the window left hidden.
                voice::bench(bench);
            } else if has_arg("--capture") {
                // A first launch with --capture starts a capture too (same as a second launch).
                screenshot::start_capture(handle.clone());
            } else if first_run || !has_arg("--hidden") {
                show_main_window(handle);
            } else {
                applog::info("started hidden (--hidden)");
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
                // neither is affected. Pinned (header pin button): it stays open. So it does
                // while Notes dictation may be running (Windows' voice typing bar takes the focus).
                WindowEvent::Focused(false)
                    if !PINNED.load(Ordering::SeqCst)
                        && !notes::keep_open()
                        && window.is_visible().unwrap_or(false)
                        && now_ms().saturating_sub(SHOWN_AT.load(Ordering::SeqCst)) > 1500 =>
                {
                    BLUR_HIDDEN_AT.store(now_ms(), Ordering::SeqCst);
                    let _ = window.hide();
                }
                _ => {}
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running HL Desk");
}
