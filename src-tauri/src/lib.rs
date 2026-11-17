mod clipboard;
mod db;
mod notes;
mod screenshot;

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

fn toggle_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
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

    tauri::Builder::default()
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
            if command.starts_with("clipboard_") {
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
            app.global_shortcut().register(toggle_shortcut())?;

            let open = MenuItem::with_id(app, "open", "Open HL Desk", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit HL Desk", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let mut tray = TrayIconBuilder::with_id("main")
                .tooltip("HL Desk")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => toggle_window(app),
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
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the main window only hides it; the app keeps running in the menu bar / tray.
            // Other windows (the screenshot editor) close normally.
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() != "main" {
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running HL Desk");
}
