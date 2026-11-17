//! Screenshot feature (Rust side). Only this file and src/features/screenshot/ belong to it.
//!
//! Capture uses macOS's own `screencapture -i` (area selection on any display, and it shows the
//! Screen Recording prompt). Elsewhere (Windows) it freezes every monitor with xcap and opens our
//! own selection overlay (overlay.html, one window per monitor, label "overlay-<n>-<i>").
//! Captures are PNG files in <app data>/screenshots/, listed in
//! screenshot.db and deleted after 24 hours. Each capture opens in its own editor window
//! (editor.html, label "editor-<id>").
//! Every capture step is written to the app log (applog.rs); a failed capture shows the main
//! window with the error on the Screenshot tab (event "screenshot-error").

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{
    image::Image,
    ipc::{Invoke, InvokeBody, Request, Response},
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::{applog, db};

/// Captures are kept for 24 hours (fixed in v1).
const KEEP_MS: i64 = 24 * 60 * 60 * 1000;
/// Sent to every window when the list of captures changes.
const CHANGED: &str = "screenshot-changed";
/// Sent to every window when a capture fails (payload: the message).
const FAILED: &str = "screenshot-error";

/// True while a capture is running, so a second shortcut press does nothing.
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// The last capture error, for a Screenshot tab that was not open yet when it was sent.
static LAST_ERROR: Mutex<Option<String>> = Mutex::new(None);

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
        applog::error(&format!("screenshot: clean-up failed: {e}"));
    }

    let registered = app
        .global_shortcut()
        .on_shortcut(capture_shortcut(), |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                applog::info("capture: shortcut pressed");
                start_capture(app.clone());
            }
        });
    match registered {
        Ok(()) => applog::info("screenshot: capture shortcut registered"),
        Err(e) => applog::error(&format!("screenshot: could not register the capture shortcut: {e}")),
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
        // Keep a capture that is open in the editor; the next clean-up removes it.
        if app.get_webview_window(&format!("editor-{id}")).is_some() {
            continue;
        }
        let _ = std::fs::remove_file(path);
        c.execute("DELETE FROM shots WHERE id = ?1", [id]).map_err(err)?;
    }
    Ok(old.len())
}

// ---------- capture ----------

/// Starts an area capture on its own thread (shortcut, Capture area button, `--capture`).
pub(crate) fn start_capture(app: AppHandle) {
    if CAPTURING.swap(true, Ordering::SeqCst) {
        applog::info("capture: already running, ignored");
        return;
    }
    *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
    applog::info("capture: start");
    std::thread::spawn(move || {
        if let Err(e) = capture(&app) {
            report_failure(&app, &e);
        }
        CAPTURING.store(false, Ordering::SeqCst);
    });
}

/// Never fail silently: log it, tell the Screenshot tab, and show the main window.
fn report_failure(app: &AppHandle, message: &str) {
    applog::error(&format!("capture: failed: {message}"));
    *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.to_string());
    let _ = app.emit(FAILED, message);
    crate::show_main_window(app);
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
        applog::info("capture: cancelled");
        return Ok(());
    }
    add_shot(app, &path, created)
}

/// Lists a new capture file in screenshot.db, tells every window, and opens it in the editor.
fn add_shot(app: &AppHandle, path: &std::path::Path, created: i64) -> Res<()> {
    let c = conn(app)?;
    c.execute(
        "INSERT INTO shots (path, created_at, updated_at) VALUES (?1, ?2, ?2)",
        (path.to_string_lossy().to_string(), created),
    )
    .map_err(err)?;
    let id = c.last_insert_rowid();
    applog::info(&format!("capture: saved shot {id} to {}", path.display()));
    let _ = app.emit(CHANGED, ());
    open_editor(app, id)?;
    applog::info(&format!("capture: done, editor-{id} opened"));
    Ok(())
}

/// Windows (and other systems): freeze every monitor, let the user drag a rectangle on our own
/// overlay, then crop that monitor's image. Blocks this capture thread until the overlay ends, so
/// CAPTURING stays set until then.
#[cfg(not(target_os = "macos"))]
fn capture(app: &AppHandle) -> Res<()> {
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    // Give the window time to disappear before the screens are frozen.
    std::thread::sleep(Duration::from_millis(250));

    let screens =
        overlay::freeze_screens().map_err(|e| format!("Could not capture the screen: {e}"))?;
    let Some(pick) = overlay::select(app, &screens)? else {
        return Ok(()); // cancelled (logged by select)
    };

    let created = now_ms();
    let path = shots_dir(app)?.join(format!("{created}.png"));
    overlay::save_crop(&screens, &pick, &path)
        .map_err(|e| format!("Could not save the capture: {e}"))?;
    add_shot(app, &path, created)
}

// ---------- selection overlay (Windows) ----------

#[cfg_attr(target_os = "macos", allow(dead_code))]
mod overlay {
    use std::{
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc::{channel, Receiver, RecvTimeoutError, Sender},
            Mutex,
        },
        time::Duration,
    };

    use tauri::{
        AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
        WebviewWindowBuilder, WindowEvent,
    };
    use xcap::image::{
        codecs::png::{CompressionType, FilterType, PngEncoder},
        imageops, ExtendedColorType, ImageEncoder, RgbaImage,
    };

    use super::{err, Res};
    use crate::applog;

    /// An overlay left open this long is closed, so a stuck page can never block captures.
    const TIMEOUT: Duration = Duration::from_secs(300);
    /// If no overlay page has shown itself by then, Rust shows the overlay windows itself.
    const SHOW_GRACE: Duration = Duration::from_secs(3);

    /// One monitor, frozen at the moment of the capture.
    pub struct Screen {
        /// Top-left corner on the virtual desktop, physical pixels.
        x: i32,
        y: i32,
        image: RgbaImage,
    }

    /// The rectangle the user dragged, in physical pixels of one monitor's image.
    pub struct Pick {
        monitor: usize,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    }

    /// What an overlay page answers: the rectangle, Err(None) when the user cancelled (Esc,
    /// right-click), or Err(Some(reason)) when the page failed.
    type Answer = Result<Pick, Option<String>>;

    /// The overlay that is open now. Dropping it without an answer (an overlay window closed)
    /// wakes the capture thread with "cancelled".
    struct Session {
        id: u64,
        pngs: Vec<PathBuf>,
        done: Sender<Answer>,
        /// An overlay page has drawn its image and shown its window.
        shown: bool,
    }

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    /// Fast PNG compression: the frozen screens are shown at once and deleted afterwards.
    fn save_png(image: &RgbaImage, path: &Path) -> Res<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path).map_err(err)?);
        PngEncoder::new_with_quality(file, CompressionType::Fast, FilterType::Adaptive)
            .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgba8)
            .map_err(err)
    }

    /// Captures every monitor (physical pixels).
    pub fn freeze_screens() -> Res<Vec<Screen>> {
        let mut screens = Vec::new();
        for m in xcap::Monitor::all().map_err(err)? {
            screens.push(Screen {
                x: m.x().map_err(err)?,
                y: m.y().map_err(err)?,
                image: m.capture_image().map_err(err)?,
            });
        }
        if screens.is_empty() {
            return Err("No screen found to capture.".into());
        }
        let sizes: Vec<String> = screens
            .iter()
            .map(|s| format!("{}x{} at ({}, {})", s.image.width(), s.image.height(), s.x, s.y))
            .collect();
        applog::info(&format!("capture: screens frozen: {}", sizes.join(", ")));
        Ok(screens)
    }

    /// Shows the overlays and waits until the user drags a rectangle (Some) or cancels (None).
    pub fn select(app: &AppHandle, screens: &[Screen]) -> Res<Option<Pick>> {
        let dir = std::env::temp_dir().join("hl-desk-capture");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(err)?;
        let pngs: Vec<PathBuf> = (0..screens.len())
            .map(|i| dir.join(format!("monitor-{i}.png")))
            .collect();
        // PNG encoding is the slow part: one thread per monitor.
        std::thread::scope(|s| {
            let jobs: Vec<_> = screens
                .iter()
                .zip(&pngs)
                .map(|(screen, png)| s.spawn(move || save_png(&screen.image, png)))
                .collect();
            jobs.into_iter().try_for_each(|job| {
                job.join()
                    .unwrap_or_else(|_| Err("PNG encoding failed".to_string()))
            })
        })
        .map_err(|e| format!("Could not prepare the capture overlay: {e}"))?;

        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        *SESSION.lock().unwrap() = Some(Session { id, pngs, done: tx, shown: false });

        let opened = screens
            .iter()
            .enumerate()
            .try_for_each(|(i, screen)| open_window(app, id, i, screen));
        let result = match opened {
            Ok(()) => {
                applog::info(&format!("capture: {} overlay window(s) opened", screens.len()));
                wait(app, id, &rx)
            }
            Err(e) => Err(format!("Could not open the capture overlay: {e}")),
        };

        // Close this session's overlays and delete the frozen PNGs.
        end(Some(id));
        for window in windows(app, id) {
            let _ = window.destroy();
        }
        let _ = std::fs::remove_dir_all(&dir);
        result
    }

    /// Waits for the overlay's answer. If no page has shown itself after SHOW_GRACE, shows the
    /// windows anyway, so the overlay never stays invisible.
    fn wait(app: &AppHandle, id: u64, rx: &Receiver<Answer>) -> Res<Option<Pick>> {
        let answer = match rx.recv_timeout(SHOW_GRACE) {
            Err(RecvTimeoutError::Timeout) => {
                if !is_shown(id) {
                    applog::warn(
                        "capture: no overlay page showed itself within 3 s; showing the overlay windows anyway",
                    );
                    for window in windows(app, id) {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
                rx.recv_timeout(TIMEOUT - SHOW_GRACE)
            }
            answer => answer,
        };
        match answer {
            Ok(Ok(pick)) => Ok(Some(pick)),
            Ok(Err(None)) => {
                applog::info("capture: cancelled by the user (Esc or right-click)");
                Ok(None)
            }
            Ok(Err(Some(reason))) => Err(reason),
            Err(RecvTimeoutError::Disconnected) => {
                applog::info("capture: cancelled (an overlay window was closed)");
                Ok(None)
            }
            Err(RecvTimeoutError::Timeout) => {
                Err("The capture overlay got no answer for 5 minutes, so it was closed.".into())
            }
        }
    }

    /// This session's overlay windows.
    fn windows(app: &AppHandle, id: u64) -> Vec<WebviewWindow> {
        let prefix = format!("overlay-{id}-");
        app.webview_windows()
            .into_iter()
            .filter(|(label, _)| label.starts_with(&prefix))
            .map(|(_, window)| window)
            .collect()
    }

    fn is_shown(id: u64) -> bool {
        matches!(SESSION.lock().unwrap().as_ref(), Some(s) if s.id == id && s.shown)
    }

    /// An overlay page showed its window. True the first time in this session.
    pub fn mark_shown() -> bool {
        match SESSION.lock().unwrap().as_mut() {
            Some(s) if !s.shown => {
                s.shown = true;
                true
            }
            _ => false,
        }
    }

    /// One borderless, always-on-top window exactly over the monitor. It stays hidden until its
    /// page has drawn the frozen image (screenshot_overlay_show).
    fn open_window(app: &AppHandle, id: u64, index: usize, screen: &Screen) -> Res<()> {
        let window = WebviewWindowBuilder::new(
            app,
            format!("overlay-{id}-{index}"),
            WebviewUrl::App(format!("overlay.html?m={index}").into()),
        )
        .title("Capture area")
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build()
        .map_err(err)?;
        // Closing any overlay another way (Alt+F4) cancels the capture.
        window.on_window_event(move |event| {
            if let WindowEvent::Destroyed = event {
                end(Some(id));
            }
        });
        // Position first, so a monitor with another scale factor applies before the size is set.
        window
            .set_position(PhysicalPosition::new(screen.x, screen.y))
            .map_err(err)?;
        window
            .set_size(PhysicalSize::new(screen.image.width(), screen.image.height()))
            .map_err(err)
    }

    /// Takes the open session (only if it is session `id`, when given). Dropping it wakes `select`.
    fn end(id: Option<u64>) -> Option<Session> {
        let mut session = SESSION.lock().unwrap();
        match (session.as_ref(), id) {
            (Some(s), Some(id)) if s.id != id => None,
            _ => session.take(),
        }
    }

    /// The frozen PNG of monitor `index`.
    pub fn png_path(index: usize) -> Res<PathBuf> {
        SESSION
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|s| s.pngs.get(index).cloned())
            .ok_or_else(|| "The capture was cancelled.".to_string())
    }

    /// The session an overlay window belongs to (its label is "overlay-<session>-<monitor>"), so
    /// a late call from an old overlay page never ends a newer capture.
    pub fn session_of(label: &str) -> Option<u64> {
        label.strip_prefix("overlay-")?.split('-').next()?.parse().ok()
    }

    /// Ends the overlay: `reason` None when the user cancelled, Some when the page failed.
    pub fn cancel(session: Option<u64>, reason: Option<String>) {
        if let Some(session) = end(session) {
            let _ = session.done.send(Err(reason));
        }
    }

    pub fn finish(session: Option<u64>, monitor: usize, x: u32, y: u32, width: u32, height: u32) {
        if let Some(session) = end(session) {
            let _ = session.done.send(Ok(Pick { monitor, x, y, width, height }));
        }
    }

    /// Crops the pick out of its monitor's image (clamped to it) and saves it as PNG.
    pub fn save_crop(screens: &[Screen], pick: &Pick, path: &Path) -> Res<()> {
        let image = &screens
            .get(pick.monitor)
            .ok_or_else(|| "Unknown monitor".to_string())?
            .image;
        let x = pick.x.min(image.width().saturating_sub(1));
        let y = pick.y.min(image.height().saturating_sub(1));
        let width = pick.width.clamp(1, image.width() - x);
        let height = pick.height.clamp(1, image.height() - y);
        save_png(&imageops::crop_imm(image, x, y, width, height).to_image(), path)
    }
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
    // Closing: the editor page saves on close-requested, then the window closes (lib.rs only
    // keeps the "main" window alive).
    let _ = window.set_focus();
    Ok(())
}

// ---------- commands ----------

/// Starts an area capture (the same as the shortcut). Returns at once; the editor opens when done.
#[tauri::command]
fn screenshot_capture(app: AppHandle) -> Res<()> {
    applog::info("capture: Capture area button");
    start_capture(app);
    Ok(())
}

/// The last capture error not shown yet (taken, so it shows once): for a Screenshot tab that
/// opens after the "screenshot-error" event was sent.
#[tauri::command]
fn screenshot_last_error() -> Option<String> {
    LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// Overlay page: the frozen PNG of its monitor.
#[tauri::command]
async fn screenshot_overlay_image(index: usize) -> Res<Response> {
    let read = overlay::png_path(index).and_then(|path| std::fs::read(path).map_err(err));
    match &read {
        Ok(bytes) => applog::info(&format!("capture: image {index} served ({} bytes)", bytes.len())),
        Err(e) => applog::error(&format!("capture: image {index} not served: {e}")),
    }
    Ok(Response::new(read?))
}

/// Overlay page: its frozen image is drawn, so show the window and give it the keyboard
/// (called again when the pointer moves onto another monitor's overlay).
#[tauri::command]
fn screenshot_overlay_show(window: WebviewWindow) {
    if overlay::mark_shown() {
        applog::info(&format!("capture: overlay shown ({})", window.label()));
    }
    let _ = window.show();
    let _ = window.set_focus();
}

/// Overlay page: Esc or right-click (no reason), or the page failed (reason, shown to the user).
#[tauri::command]
fn screenshot_overlay_cancel(window: WebviewWindow, reason: Option<String>) {
    if let Some(reason) = &reason {
        applog::error(&format!("capture: overlay page failed ({}): {reason}", window.label()));
    }
    overlay::cancel(overlay::session_of(window.label()), reason);
}

/// Overlay page: the dragged rectangle, in physical pixels of monitor `index`'s image.
#[tauri::command]
fn screenshot_overlay_done(
    window: WebviewWindow,
    index: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) {
    applog::info(&format!(
        "capture: area picked on monitor {index}: {width}x{height} at ({x}, {y})"
    ));
    overlay::finish(overlay::session_of(window.label()), index, x, y, width, height);
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
        screenshot_last_error,
        screenshot_overlay_image,
        screenshot_overlay_show,
        screenshot_overlay_cancel,
        screenshot_overlay_done,
        screenshot_list,
        screenshot_get,
        screenshot_read,
        screenshot_open,
        screenshot_save,
        screenshot_copy,
        screenshot_close
    ]
}
