//! Screenshot feature (Rust side). Only this file and src/features/screenshot/ belong to it.
//!
//! Capture uses macOS's own `screencapture -i` (area selection on any display, and it shows the
//! Screen Recording prompt). Captures are PNG files in <app data>/screenshots/, listed in
//! screenshot.db and deleted after 24 hours. Each capture opens in its own editor window
//! (editor.html, label "editor-<id>").

use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{
    image::Image,
    ipc::{Invoke, InvokeBody, Request, Response},
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::db;

/// Captures are kept for 24 hours (fixed in v1).
const KEEP_MS: i64 = 24 * 60 * 60 * 1000;
/// Sent to every window when the list of captures changes.
const CHANGED: &str = "screenshot-changed";

/// True while a capture is running, so a second shortcut press does nothing.
static CAPTURING: AtomicBool = AtomicBool::new(false);

type Res<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Cmd+Shift+2 on macOS, Ctrl+Shift+2 elsewhere.
fn capture_shortcut() -> Shortcut {
    #[cfg(target_os = "macos")]
    let main = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    let main = Modifiers::CONTROL;
    Shortcut::new(Some(main | Modifiers::SHIFT), Code::Digit2)
}

/// Called once at start-up, after the plugins are ready.
pub fn init(app: &AppHandle) {
    if let Err(e) = cleanup(app) {
        eprintln!("screenshot: clean-up failed: {e}");
    }

    let registered = app
        .global_shortcut()
        .on_shortcut(capture_shortcut(), |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                start_capture(app.clone());
            }
        });
    if let Err(e) = registered {
        eprintln!("screenshot: could not register the capture shortcut: {e}");
    }

    // Delete expired captures every 10 minutes while the app runs.
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(600));
        if matches!(cleanup(&app), Ok(n) if n > 0) {
            let _ = app.emit(CHANGED, ());
        }
    });
}

// ---------- storage ----------

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Shot {
    id: i64,
    path: String,
    /// Unix time in milliseconds.
    created_at: i64,
    /// Changes when the editor saves over the file (used to refresh thumbnails).
    updated_at: i64,
}

fn conn(app: &AppHandle) -> Res<rusqlite::Connection> {
    let c = db::open(app, "screenshot.db").map_err(err)?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS shots (
            id INTEGER PRIMARY KEY,
            path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        )",
    )
    .map_err(err)?;
    Ok(c)
}

fn shots_dir(app: &AppHandle) -> Res<PathBuf> {
    let dir = app.path().app_data_dir().map_err(err)?.join("screenshots");
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir)
}

fn row_to_shot(row: &rusqlite::Row) -> rusqlite::Result<Shot> {
    Ok(Shot {
        id: row.get(0)?,
        path: row.get(1)?,
        created_at: row.get(2)?,
        updated_at: row.get(3)?,
    })
}

fn get(app: &AppHandle, id: i64) -> Res<Shot> {
    conn(app)?
        .query_row(
            "SELECT id, path, created_at, updated_at FROM shots WHERE id = ?1",
            [id],
            row_to_shot,
        )
        .map_err(|_| "This screenshot is gone (kept 1 day).".to_string())
}

/// Deletes captures older than 24 hours (file and row). Returns how many were deleted.
fn cleanup(app: &AppHandle) -> Res<usize> {
    let c = conn(app)?;
    let cutoff = now_ms() - KEEP_MS;
    let old: Vec<(i64, String)> = {
        let mut stmt = c
            .prepare("SELECT id, path FROM shots WHERE created_at < ?1")
            .map_err(err)?;
        let rows = stmt
            .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))
            .map_err(err)?;
        rows.filter_map(Result::ok).collect()
    };
    for (id, path) in &old {
        let _ = std::fs::remove_file(path);
        c.execute("DELETE FROM shots WHERE id = ?1", [id]).map_err(err)?;
    }
    Ok(old.len())
}

// ---------- capture ----------

fn start_capture(app: AppHandle) {
    if CAPTURING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || {
        if let Err(e) = capture(&app) {
            eprintln!("screenshot: capture failed: {e}");
        }
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

#[cfg(target_os = "macos")]
fn capture(app: &AppHandle) -> Res<()> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    // Give the window time to disappear before the crosshair shows.
    std::thread::sleep(Duration::from_millis(250));

    let created = now_ms();
    let path = shots_dir(app)?.join(format!("{created}.png"));
    // -i: let the user drag an area (Space toggles window mode, Esc cancels); -x: no sound.
    std::process::Command::new("/usr/sbin/screencapture")
        .arg("-i")
        .arg("-x")
        .arg(&path)
        .status()
        .map_err(err)?;
    if !path.exists() {
        return Ok(()); // cancelled
    }

    let c = conn(app)?;
    c.execute(
        "INSERT INTO shots (path, created_at, updated_at) VALUES (?1, ?2, ?2)",
        (path.to_string_lossy().to_string(), created),
    )
    .map_err(err)?;
    let id = c.last_insert_rowid();
    let _ = app.emit(CHANGED, ());
    open_editor(app, id)
}

#[cfg(not(target_os = "macos"))]
fn capture(_app: &AppHandle) -> Res<()> {
    Err("Area capture is macOS-only for now.".into())
}

// ---------- editor window ----------

/// Width and height in pixels, read from the PNG header.
fn png_size(path: &str) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut head = [0u8; 24];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..8] != b"\x89PNG\r\n\x1a\n" {
        return None;
    }
    let w = u32::from_be_bytes(head[16..20].try_into().ok()?);
    let h = u32::from_be_bytes(head[20..24].try_into().ok()?);
    Some((w, h))
}

/// Window size (logical points) that shows the image at its natural size, shrunk to fit the screen.
fn editor_size(app: &AppHandle, px_w: u32, px_h: u32) -> (f64, f64) {
    const SIDE: f64 = 140.0; // "Drag me" column
    const BARS: f64 = 100.0; // toolbar + footer
    let (scale, max_w, max_h) = match app.primary_monitor() {
        Ok(Some(m)) => {
            let s = m.scale_factor();
            let size = m.size().to_logical::<f64>(s);
            (s, size.width * 0.9, size.height * 0.85)
        }
        _ => (1.0, 1280.0, 800.0),
    };
    let img_w = px_w as f64 / scale;
    let img_h = px_h as f64 / scale;
    let fit = ((max_w - SIDE) / img_w).min((max_h - BARS) / img_h).min(1.0);
    ((img_w * fit + SIDE).max(600.0), (img_h * fit + BARS).max(380.0))
}

fn open_editor(app: &AppHandle, id: i64) -> Res<()> {
    let label = format!("editor-{id}");
    if let Some(window) = app.get_webview_window(&label) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return Ok(());
    }

    let shot = get(app, id)?;
    let (w, h) = png_size(&shot.path).unwrap_or((1600, 1000));
    let (win_w, win_h) = editor_size(app, w, h);
    let window = WebviewWindowBuilder::new(
        app,
        &label,
        WebviewUrl::App(format!("editor.html?id={id}").into()),
    )
    .title("Screenshot")
    .inner_size(win_w, win_h)
    .min_inner_size(600.0, 380.0)
    .center()
    .focused(true)
    .build()
    .map_err(err)?;

    // lib.rs hides every window on close (right for the popover); an editor is closed for good.
    let this = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { .. } = event {
            let _ = this.destroy();
        }
    });
    let _ = window.set_focus();
    Ok(())
}

// ---------- commands ----------

/// Starts an area capture (the same as the shortcut). Returns at once; the editor opens when done.
#[tauri::command]
fn screenshot_capture(app: AppHandle) -> Res<()> {
    if cfg!(not(target_os = "macos")) {
        return Err("Area capture is macOS-only for now.".into());
    }
    start_capture(app);
    Ok(())
}

/// Recent captures, newest first (expired ones are deleted first).
#[tauri::command]
fn screenshot_list(app: AppHandle) -> Res<Vec<Shot>> {
    cleanup(&app)?;
    let c = conn(&app)?;
    let mut stmt = c
        .prepare("SELECT id, path, created_at, updated_at FROM shots ORDER BY created_at DESC")
        .map_err(err)?;
    let rows = stmt.query_map([], row_to_shot).map_err(err)?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[tauri::command]
fn screenshot_get(app: AppHandle, id: i64) -> Res<Shot> {
    get(&app, id)
}

/// The PNG bytes of a capture (arrives in JS as an ArrayBuffer).
#[tauri::command]
async fn screenshot_read(app: AppHandle, id: i64) -> Res<Response> {
    let shot = get(&app, id)?;
    let bytes = std::fs::read(&shot.path).map_err(err)?;
    Ok(Response::new(bytes))
}

/// Opens a capture in the editor and hides the popover (it stays on top otherwise).
#[tauri::command]
async fn screenshot_open(app: AppHandle, id: i64) -> Res<()> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    open_editor(&app, id)
}

fn header<T: std::str::FromStr>(request: &Request<'_>, name: &str) -> Res<T> {
    request
        .headers()
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| format!("missing header {name}"))
}

/// Saves the edited PNG (raw request body) over the capture. Header x-shot-id.
#[tauri::command]
fn screenshot_save(app: AppHandle, request: Request<'_>) -> Res<()> {
    let id: i64 = header(&request, "x-shot-id")?;
    let InvokeBody::Raw(png) = request.body() else {
        return Err("expected PNG bytes".into());
    };
    let shot = get(&app, id)?;
    std::fs::write(&shot.path, png).map_err(err)?;
    conn(&app)?
        .execute(
            "UPDATE shots SET updated_at = ?1 WHERE id = ?2",
            (now_ms(), id),
        )
        .map_err(err)?;
    let _ = app.emit(CHANGED, ());
    Ok(())
}

/// Copies an image to the clipboard. Body: raw RGBA pixels; headers x-width and x-height.
#[tauri::command]
fn screenshot_copy(app: AppHandle, request: Request<'_>) -> Res<()> {
    let width: u32 = header(&request, "x-width")?;
    let height: u32 = header(&request, "x-height")?;
    let InvokeBody::Raw(rgba) = request.body() else {
        return Err("expected RGBA bytes".into());
    };
    if rgba.len() != width as usize * height as usize * 4 {
        return Err("image size does not match".into());
    }
    app.clipboard()
        .write_image(&Image::new(rgba, width, height))
        .map_err(err)
}

/// Closes the editor window that calls it.
#[tauri::command]
fn screenshot_close(window: WebviewWindow) {
    let _ = window.destroy();
}

/// This feature's commands. Name each one "screenshot_<name>" (for example "screenshot_list")
/// so lib.rs routes it here, and list it in generate_handler! below.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        screenshot_capture,
        screenshot_list,
        screenshot_get,
        screenshot_read,
        screenshot_open,
        screenshot_save,
        screenshot_copy,
        screenshot_close
    ]
}
