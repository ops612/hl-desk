//! The toast: a small borderless window in a corner of the main screen (toast.html, src/toast/,
//! label "toast-<n>", capability toast.json). Windows: top-right of the work area, because the
//! bottom-right corner is where Windows' own notifications ("Snip saved to clipboard") appear and
//! cover it. macOS: bottom-right (its notifications are top-right).
//! It never takes the keyboard, so a Ctrl+V right after a screenshot still goes to the app the user
//! was in. Kinds: "cancelled" (a capture was cancelled, about 2 s) and "shot" (a device screenshot
//! with a "Mark up in HL Desk" button, about 8 s, longer while the pointer is over it). Only one
//! toast at a time.
//!
//! Windows: the window is built hidden and without the undecorated shadow; its page calls
//! screenshot_toast_show once it has drawn, and Rust then shows it on top without activating it.
//! (Built visible and never activated, the web view stayed blank white on a real Windows 10 PC.)
//! A page that has not reported within SHOW_GRACE is logged and closed, never left blank.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::applog;

static NEXT: AtomicU64 = AtomicU64::new(1);
/// The newest toast whose page reported it has drawn (screenshot_toast_show).
static SHOWN: AtomicU64 = AtomicU64::new(0);
/// A toast page that has not reported by then is logged and closed.
const SHOW_GRACE: Duration = Duration::from_secs(3);
/// The device screenshot behind the "Mark up in HL Desk" toast (PNG bytes).
static PENDING: Mutex<Option<Vec<u8>>> = Mutex::new(None);

fn pending_lock() -> std::sync::MutexGuard<'static, Option<Vec<u8>>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// Closes every open toast. True if one was open.
pub fn close_all(app: &AppHandle) -> bool {
    let mut any = false;
    for (label, window) in app.webview_windows() {
        if label.starts_with("toast-") {
            let _ = window.destroy();
            any = true;
        }
    }
    any
}

/// A short notice ("cancelled": "Capture cancelled").
pub fn notice(app: &AppHandle, kind: &str) {
    open(app, &format!("toast.html?kind={kind}"), 240.0, 48.0);
}

/// The "Mark up in HL Desk" offer for a device screenshot (PNG bytes).
pub fn offer(app: &AppHandle, png: Vec<u8>) {
    *pending_lock() = Some(png);
    open(app, "toast.html?kind=shot", 360.0, 96.0);
}

/// The offered image, left in place (for the toast's thumbnail).
pub fn pending() -> Option<Vec<u8>> {
    pending_lock().clone()
}

/// The offered image, taken (for "Mark up in HL Desk").
pub fn take_pending() -> Option<Vec<u8>> {
    pending_lock().take()
}

/// The toast page closed itself (its x, or its timer ran out).
pub fn closed(window: &WebviewWindow) {
    if window.url().is_ok_and(|u| u.query().unwrap_or("").contains("kind=shot")) {
        *pending_lock() = None;
    }
    let _ = window.destroy();
}

/// The toast's number (its label is "toast-<n>").
fn number(window: &WebviewWindow) -> u64 {
    window.label().strip_prefix("toast-").and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// The toast page has drawn: show the window on top, without taking the focus.
pub fn ready(window: &WebviewWindow) {
    SHOWN.fetch_max(number(window), Ordering::SeqCst);
    applog::info(&format!("toast: {} page ready, shown", window.label()));
    show_without_focus(window);
}

/// Windows: SetWindowPos with SWP_NOACTIVATE (Tauri's show() would activate the window and take
/// the keyboard from the user's app). Elsewhere the toast is built visible.
#[cfg(windows)]
fn show_without_focus(window: &WebviewWindow) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
    };
    match window.hwnd() {
        Ok(hwnd) => {
            let ok = unsafe {
                SetWindowPos(
                    hwnd.0 as _,
                    HWND_TOPMOST,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                )
            };
            if ok == 0 {
                applog::error(&format!("toast: could not show {}", window.label()));
            }
        }
        Err(e) => applog::error(&format!("toast: no window handle for {}: {e}", window.label())),
    }
}

#[cfg(not(windows))]
fn show_without_focus(_window: &WebviewWindow) {}

/// Where a `w` x `h` point toast goes on the main screen's work area (logical points).
/// Windows: top-right (Windows' own notifications use the bottom-right corner).
/// macOS: bottom-right, below the menu bar (its notifications use the top-right corner).
fn position(app: &AppHandle, w: f64, h: f64) -> (f64, f64) {
    const MARGIN: f64 = 16.0;
    match app.primary_monitor() {
        Ok(Some(m)) => {
            let scale = m.scale_factor();
            let area = m.work_area();
            let right = (area.position.x as f64 + area.size.width as f64) / scale;
            if cfg!(windows) {
                (right - w - MARGIN, area.position.y as f64 / scale + MARGIN)
            } else {
                let bottom = (area.position.y as f64 + area.size.height as f64) / scale;
                (right - w - MARGIN, bottom - h - MARGIN)
            }
        }
        _ => (40.0, 40.0),
    }
}

/// Opens a toast of `w` x `h` points (see `position`), always on top, without taking the focus.
fn open(app: &AppHandle, url: &str, w: f64, h: f64) {
    close_all(app);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let label = format!("toast-{n}");
    let (x, y) = position(app, w, h);
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("HL Desk")
        .inner_size(w, h)
        .position(x, y)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false);
    // Windows: hidden until the page has drawn (ready), no undecorated-shadow frame (the page
    // draws its own 1px border). Elsewhere: built visible, as a later show() could take the focus.
    #[cfg(windows)]
    let builder = builder.visible(false).shadow(false);
    #[cfg(not(windows))]
    let builder = builder.visible(true).shadow(true);
    match builder.build() {
        Ok(_) => applog::info(&format!("toast: {label} opened ({url})")),
        Err(e) => {
            applog::error(&format!("toast: could not open {url}: {e}"));
            return;
        }
    }

    // Never leave a toast that did not draw: log it and close it.
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOW_GRACE);
        if SHOWN.load(Ordering::SeqCst) >= n {
            return;
        }
        if let Some(window) = app.get_webview_window(&label) {
            applog::error(&format!(
                "toast: {label} page did not report ready within {} s; closed",
                SHOW_GRACE.as_secs()
            ));
            closed(&window);
        }
    });
}
