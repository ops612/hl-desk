//! The toast: a small borderless window in the bottom-right corner of the main screen
//! (toast.html, src/toast/, label "toast-<n>", capability toast.json). It never takes the
//! keyboard, so a Ctrl+V right after a screenshot still goes to the app the user was in.
//! Kinds: "cancelled" (a capture was cancelled, about 2 s) and "shot" (a device screenshot with
//! a "Mark up in HL Desk" button, about 6 s). Only one toast at a time.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::applog;

static NEXT: AtomicU64 = AtomicU64::new(1);
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

/// Opens a toast of `w` x `h` points in the bottom-right corner of the main screen's work area
/// (above the Windows taskbar, below the macOS menu bar), without taking the focus.
fn open(app: &AppHandle, url: &str, w: f64, h: f64) {
    close_all(app);
    let label = format!("toast-{}", NEXT.fetch_add(1, Ordering::SeqCst));
    let (x, y) = match app.primary_monitor() {
        Ok(Some(m)) => {
            let scale = m.scale_factor();
            let area = m.work_area();
            let right = (area.position.x as f64 + area.size.width as f64) / scale;
            let bottom = (area.position.y as f64 + area.size.height as f64) / scale;
            (right - w - 16.0, bottom - h - 16.0)
        }
        _ => (40.0, 40.0),
    };
    // Built visible: a later show() could take the focus away from the user's app. The page is
    // white like the window, so there is no flash while it loads.
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("HL Desk")
        .inner_size(w, h)
        .position(x, y)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(true)
        .focused(false)
        .visible(true)
        .build();
    match built {
        Ok(_) => applog::info(&format!("toast: {label} opened ({url})")),
        Err(e) => applog::error(&format!("toast: could not open {url}: {e}")),
    }
}
