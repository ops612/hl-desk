//! Screenshot feature (Rust side). Only this file and src/features/screenshot/ belong to it.
//!
//! Capture uses macOS's own `screencapture -i` (area selection on any display, and it shows the
//! Screen Recording prompt). Elsewhere (Windows) it freezes every monitor with xcap and opens our
//! own selection overlay (overlay.html, one window per monitor, label "overlay-<n>-<i>").
//! Captures are PNG files in <app data>/screenshots/, listed in
//! screenshot.db and deleted after 24 hours. Each capture opens in its own editor window
//! (editor.html, label "editor-<id>").
//! Every capture step is written to the app log (applog.rs); a failed capture shows the main
//! window with the error on the Screenshot tab (event "screenshot-error"), and a cancelled one
//! shows a short "Capture cancelled" toast (screenshot/toast.rs).
//!
//! Sub-modules (files in src-tauri/src/screenshot/):
//! - keys.rs: the capture keys (Ctrl/Cmd+Shift+2 always, plus Print Screen / Cmd+Shift+4 as a setting)
//! - watch.rs: "Mark up in HL Desk" after a device screenshot (Windows clipboard, macOS folder)
//! - toast.rs: the small corner window for notices and the "Mark up in HL Desk" offer

mod keys;
mod toast;
mod watch;

use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{
    image::Image,
    ipc::{Invoke, InvokeBody, Request, Response},
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use xcap::image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    ExtendedColorType, ImageEncoder,
};

use crate::{applog, db};

/// Captures are kept for 24 hours (fixed in v1).
const KEEP_MS: i64 = 24 * 60 * 60 * 1000;
/// Sent to every window when the list of captures changes.
const CHANGED: &str = "screenshot-changed";
/// Sent to every window when a capture fails (payload: the message).
const FAILED: &str = "screenshot-error";
/// Sent when a capture stopped because macOS has not allowed screen recording yet.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const PERMISSION: &str = "screenshot-permission";

/// Sent to every window when the capture key setting changes (the Screenshot tab shows the keys).
const KEYS_CHANGED: &str = "screenshot-keys-changed";

/// True while a capture is running; a second press of the capture key then cancels it.
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// The capture key was pressed again before the overlay was open: cancel it as soon as it opens.
#[cfg_attr(target_os = "macos", allow(dead_code))]
static CANCEL_PENDING: AtomicBool = AtomicBool::new(false);
/// When the current capture started (for the timings in the log).
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
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

/// Milliseconds since the current capture started (0 when none is running).
fn elapsed_ms() -> u128 {
    STARTED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map_or(0, |t| t.elapsed().as_millis())
}

/// PNG bytes of an RGBA image (fast compression).
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn png_bytes(rgba: &[u8], width: u32, height: u32) -> Res<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(err)?;
    Ok(out)
}

/// Called once at start-up, after the plugins are ready.
pub fn init(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    permission::init();
    if let Err(e) = cleanup(app) {
        applog::error(&format!("screenshot: clean-up failed: {e}"));
    }

    // Ctrl/Cmd+Shift+2 always, plus Print Screen / Cmd+Shift+4 when chosen in Settings.
    keys::init(app);
    // "Mark up in HL Desk" after a device screenshot (Win+Shift+S, Cmd+Shift+4 ...).
    watch::init(app);

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

/// How a capture ended without an error.
enum Outcome {
    /// Saved and opened in the editor, or stopped for a reason already shown (macOS permission card).
    Done,
    /// The user cancelled (Esc, right-click, the capture key again): a "Capture cancelled" toast.
    Cancelled,
}

/// Starts an area capture on its own thread (shortcut, Capture area button, `--capture`).
/// While one is open, a second call (the capture key pressed again) cancels it.
pub(crate) fn start_capture(app: AppHandle) {
    if CAPTURING.swap(true, Ordering::SeqCst) {
        #[cfg(target_os = "macos")]
        applog::info("capture: already running (screencapture is open; Esc cancels it), ignored");
        #[cfg(not(target_os = "macos"))]
        {
            applog::info("capture: pressed again while a capture is open, cancelling it");
            CANCEL_PENDING.store(true, Ordering::SeqCst);
            overlay::cancel(None, None);
        }
        return;
    }
    CANCEL_PENDING.store(false, Ordering::SeqCst);
    *STARTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = None;
    applog::info("capture: start");
    std::thread::spawn(move || {
        match capture(&app) {
            Ok(Outcome::Done) => {}
            Ok(Outcome::Cancelled) => toast::notice(&app, "cancelled"),
            Err(e) => report_failure(&app, &e),
        }
        // Windows: capture() ended the capture itself when the overlay closed.
        #[cfg(target_os = "macos")]
        end_capturing();
    });
}

/// The capture is over for the capture key: a press from now on starts a new one.
fn end_capturing() {
    CANCEL_PENDING.store(false, Ordering::SeqCst);
    CAPTURING.store(false, Ordering::SeqCst);
}

/// Hides the popover and any toast before the screen is captured, waiting only if one was showing.
fn clear_screen_for_capture(app: &AppHandle) {
    let mut wait = toast::close_all(app);
    if let Some(main) = app.get_webview_window("main") {
        if main.is_visible().unwrap_or(false) {
            let _ = main.hide();
            wait = true;
        }
    }
    if wait {
        // Give the windows time to disappear before the screen is captured.
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Never fail silently: log it, tell the Screenshot tab, and show the main window.
fn report_failure(app: &AppHandle, message: &str) {
    applog::error(&format!("capture: failed: {message}"));
    *LAST_ERROR.lock().unwrap_or_else(|e| e.into_inner()) = Some(message.to_string());
    let _ = app.emit(FAILED, message);
    crate::show_main_window(app);
}

/// macOS Screen Recording permission (CoreGraphics, macOS 10.15+). Without it `screencapture`
/// saves only the wallpaper, so a capture checks first.
#[cfg(target_os = "macos")]
mod permission {
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    /// True when screen recording is allowed (no prompt).
    pub fn granted() -> bool {
        unsafe { CGPreflightScreenCaptureAccess() }
    }

    /// Asks for it: the first time, macOS shows its prompt and adds HL Desk to the list in
    /// System Settings. Returns true only if it is allowed already.
    pub fn request() -> bool {
        ASKED.store(true, Ordering::SeqCst);
        unsafe { CGRequestScreenCaptureAccess() }
    }

    use std::sync::atomic::{AtomicBool, Ordering};

    /// Allowed when this run started. Allowed only later means macOS's own "Quit & Reopen" was
    /// skipped: the card then offers "Restart HL Desk".
    static AT_START: AtomicBool = AtomicBool::new(false);
    /// request() was called in this run (it lists HL Desk in System Settings).
    static ASKED: AtomicBool = AtomicBool::new(false);

    /// Called once at start-up.
    pub fn init() {
        AT_START.store(granted(), Ordering::SeqCst);
    }

    /// "granted", "restart" (allowed after this run started) or "denied". Still denied when the
    /// user comes back: ask once more per app start, so HL Desk is surely in the list.
    pub fn status() -> &'static str {
        if granted() {
            return if AT_START.load(Ordering::SeqCst) { "granted" } else { "restart" };
        }
        if !ASKED.load(Ordering::SeqCst) {
            let allowed = request();
            crate::applog::info(&format!("permission: asked again (this run's first request), returned {allowed}"));
        }
        "denied"
    }
}

/// System Settings > Privacy & Security > Screen Recording.
#[cfg(target_os = "macos")]
const SCREEN_RECORDING_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture";

#[cfg(target_os = "macos")]
fn capture(app: &AppHandle) -> Res<Outcome> {
    if permission::granted() {
        applog::info("capture: screen recording allowed");
    } else {
        applog::info("capture: screen recording not allowed (preflight), asking");
        let allowed = permission::request();
        applog::info(&format!("capture: request returned {allowed}"));
        if !allowed {
            // Do not capture a wallpaper-only image: show the permission card instead.
            let _ = app.emit(PERMISSION, ());
            crate::show_main_window(app);
            applog::info("capture: stopped, permission card shown on the Screenshot tab");
            return Ok(Outcome::Done);
        }
    }
    clear_screen_for_capture(app);

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
        return Ok(Outcome::Cancelled);
    }
    add_shot(app, &path, created)?;
    Ok(Outcome::Done)
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

/// Writes an image that came from elsewhere (a device screenshot) into the shot storage and
/// opens it in the editor, like a capture.
fn import_png(app: &AppHandle, png: &[u8]) -> Res<()> {
    let created = now_ms();
    let path = shots_dir(app)?.join(format!("{created}.png"));
    std::fs::write(&path, png).map_err(err)?;
    applog::info(&format!("device shot: imported ({} bytes)", png.len()));
    add_shot(app, &path, created)
}

/// Windows (and other systems): freeze every monitor, let the user drag a rectangle on our own
/// overlay, then crop that monitor's image. CAPTURING stays set while the overlay is open and is
/// cleared as soon as it closes (before saving and opening the editor), so a key press then
/// starts a new capture instead of being ignored.
#[cfg(not(target_os = "macos"))]
fn capture(app: &AppHandle) -> Res<Outcome> {
    let selection = (|| {
        clear_screen_for_capture(app);
        let screens = std::sync::Arc::new(
            overlay::freeze_screens().map_err(|e| format!("Could not capture the screen: {e}"))?,
        );
        applog::info(&format!("capture: screens frozen after {} ms", elapsed_ms()));
        let pick = overlay::select(app, &screens)?;
        Ok::<_, String>(pick.map(|pick| (screens, pick)))
    })();
    end_capturing();
    let Some((screens, pick)) = selection? else {
        return Ok(Outcome::Cancelled); // logged by select
    };

    let created = now_ms();
    let path = shots_dir(app)?.join(format!("{created}.png"));
    overlay::save_crop(&screens, &pick, &path)
        .map_err(|e| format!("Could not save the capture: {e}"))?;
    add_shot(app, &path, created)?;
    Ok(Outcome::Done)
}

// ---------- selection overlay (Windows) ----------

#[cfg_attr(target_os = "macos", allow(dead_code))]
mod overlay {
    use std::{
        path::Path,
        sync::{
            atomic::{AtomicU64, Ordering},
            mpsc::{channel, Receiver, RecvTimeoutError, Sender},
            Arc, Mutex,
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

    use super::{elapsed_ms, err, Res, CANCEL_PENDING};
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
        /// The frozen monitors; each overlay page fetches its own as raw RGBA pixels.
        screens: Arc<Vec<Screen>>,
        done: Sender<Answer>,
        /// An overlay page has drawn its image and shown its window.
        shown: bool,
    }

    static SESSION: Mutex<Option<Session>> = Mutex::new(None);
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);

    /// Fast PNG compression.
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
    /// The windows are transparent: each page shows its dim layer and hint at once, over the
    /// (still unchanged) screen, then draws the frozen pixels when they arrive, so the user never
    /// waits for image encoding before the capture visibly starts.
    pub fn select(app: &AppHandle, screens: &Arc<Vec<Screen>>) -> Res<Option<Pick>> {
        let id = NEXT_ID.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = channel();
        *SESSION.lock().unwrap() =
            Some(Session { id, screens: Arc::clone(screens), done: tx, shown: false });
        // The capture key was pressed again while the screens were being frozen.
        if CANCEL_PENDING.swap(false, Ordering::SeqCst) {
            cancel(Some(id), None);
        }

        let opened = if is_open(id) {
            screens
                .iter()
                .enumerate()
                .try_for_each(|(i, screen)| open_window(app, id, i, screen))
        } else {
            Ok(())
        };
        let result = match opened {
            Ok(()) => {
                applog::info(&format!(
                    "capture: {} overlay window(s) opened after {} ms",
                    screens.len(),
                    elapsed_ms()
                ));
                wait(app, id, &rx)
            }
            Err(e) => Err(format!("Could not open the capture overlay: {e}")),
        };

        // Close this session's overlays.
        end(Some(id));
        for window in windows(app, id) {
            let _ = window.destroy();
        }
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
                applog::info("capture: cancelled by the user (Esc, right-click or the capture key)");
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

    fn is_open(id: u64) -> bool {
        matches!(SESSION.lock().unwrap().as_ref(), Some(s) if s.id == id)
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

    /// One borderless, always-on-top, transparent window exactly over the monitor. It stays
    /// hidden until its page is ready to draw (screenshot_overlay_show), so there is no white flash.
    fn open_window(app: &AppHandle, id: u64, index: usize, screen: &Screen) -> Res<()> {
        let (w, h) = (screen.image.width(), screen.image.height());
        let builder = WebviewWindowBuilder::new(
            app,
            format!("overlay-{id}-{index}"),
            WebviewUrl::App(format!("overlay.html?m={index}&w={w}&h={h}").into()),
        );
        // Transparent until the frozen pixels arrive (macOS never uses this overlay).
        #[cfg(not(target_os = "macos"))]
        let builder = builder.transparent(true);
        let window = builder
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

    /// The frozen pixels of monitor `index` (RGBA; width and height are in the page's URL).
    pub fn frame(index: usize) -> Res<Vec<u8>> {
        let screens = SESSION
            .lock()
            .unwrap()
            .as_ref()
            .map(|s| Arc::clone(&s.screens))
            .ok_or_else(|| "The capture was cancelled.".to_string())?;
        let screen = screens.get(index).ok_or_else(|| format!("no monitor {index}"))?;
        Ok(screen.image.as_raw().clone())
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
    const SIDE: f64 = 24.0; // canvas padding
    const BARS: f64 = 130.0; // toolbar + bottom bar (Drag to share, Copy, Done)
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

/// Screen capture permission: "granted", "restart" (macOS: allowed after HL Desk started, so it
/// must restart first) or "denied" (macOS; asks once more per app start). Always "granted"
/// elsewhere (only macOS asks).
#[tauri::command]
fn screenshot_permission_status() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        let status = permission::status();
        applog::info(&format!("permission: screen recording {status}"));
        status
    }
    #[cfg(not(target_os = "macos"))]
    {
        "granted"
    }
}

/// macOS "One more step": restarts HL Desk, so a screen recording grant made after it started
/// takes effect (macOS's own "Quit & Reopen" was skipped).
#[tauri::command]
fn screenshot_restart(app: AppHandle) {
    applog::info("permission: restarting HL Desk so screen recording applies");
    app.request_restart();
}

/// macOS: asks for screen recording (adds HL Desk to the list the first time) and opens
/// System Settings at Privacy & Security > Screen Recording. Does nothing elsewhere.
#[tauri::command]
fn screenshot_open_permission_settings() -> Res<()> {
    #[cfg(target_os = "macos")]
    {
        if !permission::granted() {
            let allowed = permission::request();
            applog::info(&format!("permission: request returned {allowed}"));
        }
        std::process::Command::new("/usr/bin/open")
            .arg(SCREEN_RECORDING_PANE)
            .status()
            .map_err(err)?;
        applog::info("permission: opened System Settings > Screen Recording");
    }
    Ok(())
}

/// Overlay page: the frozen pixels of its monitor (raw RGBA, no encoding, so they arrive fast).
#[tauri::command]
async fn screenshot_overlay_image(index: usize) -> Res<Response> {
    let read = overlay::frame(index);
    match &read {
        Ok(bytes) => applog::info(&format!(
            "capture: image {index} served ({} bytes) after {} ms",
            bytes.len(),
            elapsed_ms()
        )),
        Err(e) => applog::error(&format!("capture: image {index} not served: {e}")),
    }
    Ok(Response::new(read?))
}

/// Overlay page: it is ready (dim layer and hint drawn), so show the window and give it the
/// keyboard (called again when the pointer moves onto another monitor's overlay).
#[tauri::command]
fn screenshot_overlay_show(window: WebviewWindow) {
    if overlay::mark_shown() {
        applog::info(&format!(
            "capture: overlay shown ({}) after {} ms",
            window.label(),
            elapsed_ms()
        ));
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
    // Our own copy: the device-screenshot watcher must not offer it back.
    watch::mark_own_write();
    app.clipboard()
        .write_image(&Image::new(rgba, width, height))
        .map_err(err)
}

/// Closes the editor window that calls it.
#[tauri::command]
fn screenshot_close(window: WebviewWindow) {
    let _ = window.destroy();
}

// ---------- capture settings (Settings sheet, src/features/screenshot/settings.ts) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CaptureSettings {
    /// "default" or "device".
    key: String,
    /// The device key (Print Screen / Cmd+Shift+4): "off", "ok", "owned" (the OS still uses it),
    /// "check" (could not tell) or "failed" (could not register it).
    device_key: &'static str,
    /// "Mark up in HL Desk" after a device screenshot.
    markup: bool,
    /// macOS: mark up is on but the screenshot folder cannot be read (Desktop access refused).
    folder_blocked: bool,
}

fn capture_settings(app: &AppHandle) -> CaptureSettings {
    let markup = crate::settings::markup_after_shot(app);
    CaptureSettings {
        key: crate::settings::capture_key(app),
        device_key: keys::status(app),
        markup,
        folder_blocked: markup && watch::folder_blocked(),
    }
}

#[tauri::command]
async fn screenshot_capture_settings(app: AppHandle) -> CaptureSettings {
    capture_settings(&app)
}

/// Chooses the capture key ("default" or "device"), registers it and returns the new state.
#[tauri::command]
async fn screenshot_set_capture_key(app: AppHandle, key: String) -> Res<CaptureSettings> {
    if key != keys::DEFAULT && key != keys::DEVICE {
        return Err(format!("unknown capture key {key}"));
    }
    crate::settings::set_capture_key(&app, &key)?;
    applog::info(&format!("settings: capture key is now {key}"));
    keys::apply(&app, &key);
    let _ = app.emit(KEYS_CHANGED, ());
    Ok(capture_settings(&app))
}

/// Turns "Mark up in HL Desk" after a device screenshot on or off. macOS, turning it on: looks
/// at the screenshot folder once first (macOS asks for Desktop access and this waits for the
/// answer); the result shows as `folder_blocked`.
#[tauri::command]
async fn screenshot_set_markup(app: AppHandle, on: bool) -> Res<CaptureSettings> {
    crate::settings::set_markup_after_shot(&app, on)?;
    #[cfg(target_os = "macos")]
    {
        if on {
            watch::check_folder();
        }
    }
    watch::set_on(on);
    applog::info(&format!("settings: mark up after a device screenshot is now {on}"));
    Ok(capture_settings(&app))
}

/// System Settings > Privacy & Security > Files and Folders.
#[cfg(target_os = "macos")]
const FILES_AND_FOLDERS_PANE: &str =
    "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders";

/// macOS: opens System Settings at Files and Folders (screenshot folder access). Does nothing elsewhere.
#[tauri::command]
fn screenshot_open_files_settings() -> Res<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("/usr/bin/open")
            .arg(FILES_AND_FOLDERS_PANE)
            .status()
            .map_err(err)?;
        applog::info("device shot: opened System Settings > Files and Folders");
    }
    Ok(())
}

/// Opens the system setting that owns the device key (Windows: Accessibility > Keyboard;
/// macOS: Keyboard settings, where Keyboard Shortcuts > Screenshots is).
#[tauri::command]
fn screenshot_open_key_settings() -> Res<()> {
    keys::open_system_settings()
}

// ---------- toast (toast.html, src/toast/) ----------

/// Toast page: the image behind "Mark up in HL Desk" (PNG bytes).
#[tauri::command]
async fn screenshot_toast_image() -> Res<Response> {
    toast::pending()
        .map(Response::new)
        .ok_or_else(|| "The screenshot is no longer available.".to_string())
}

/// Toast page: "Mark up in HL Desk": stores the image like a capture and opens the editor.
#[tauri::command]
async fn screenshot_toast_markup(app: AppHandle, window: WebviewWindow) -> Res<()> {
    applog::info("device shot: Mark up in HL Desk clicked");
    let result = toast::take_pending()
        .ok_or_else(|| "The screenshot is no longer available.".to_string())
        .and_then(|png| import_png(&app, &png));
    // Never fail silently: the main window shows the error before the toast goes.
    if let Err(e) = &result {
        report_failure(&app, &format!("Could not open the screenshot in the editor: {e}"));
    }
    let _ = window.destroy();
    result
}

/// Toast page: it has drawn and its content is `height` CSS pixels tall, so size the window to
/// it and show it (on top, without taking the focus).
#[tauri::command]
fn screenshot_toast_show(window: WebviewWindow, height: f64) {
    toast::ready(&window, height);
}

/// Toast page: closed (its x, or its timer ran out).
#[tauri::command]
fn screenshot_toast_close(window: WebviewWindow) {
    toast::closed(&window);
}

/// This feature's commands. Name each one "screenshot_<name>" (for example "screenshot_list")
/// so lib.rs routes it here, and list it in generate_handler! below.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        screenshot_capture,
        screenshot_last_error,
        screenshot_permission_status,
        screenshot_restart,
        screenshot_open_permission_settings,
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
        screenshot_close,
        screenshot_capture_settings,
        screenshot_set_capture_key,
        screenshot_set_markup,
        screenshot_open_files_settings,
        screenshot_open_key_settings,
        screenshot_toast_image,
        screenshot_toast_markup,
        screenshot_toast_show,
        screenshot_toast_close
    ]
}
