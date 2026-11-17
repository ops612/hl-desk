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
//! screenshot_toast_show with its content height once it has drawn, and Rust then sizes, places
//! and shows it on top without activating it, with one SetWindowPos in physical pixels (the
//! builder's logical inner size and position left the card cut off at the top on a real Windows
//! 10 PC), and nudges the height 1 px and back so the web view repaints once visible.
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

/// Toast widths in points; the height comes from the page (screenshot_toast_show).
const NOTICE_W: f64 = 240.0;
const SHOT_W: f64 = 360.0;
/// Heights used until the page reports its own (what toast.css draws).
const NOTICE_H: f64 = 48.0;
const SHOT_H: f64 = 96.0;
/// Gap between the toast and the work area's edges, in points.
const MARGIN: f64 = 16.0;

/// A short notice ("cancelled": "Capture cancelled").
pub fn notice(app: &AppHandle, kind: &str) {
    open(app, &format!("toast.html?kind={kind}"), NOTICE_W, NOTICE_H);
}

/// The "Mark up in HL Desk" offer for a device screenshot (PNG bytes).
pub fn offer(app: &AppHandle, png: Vec<u8>) {
    *pending_lock() = Some(png);
    open(app, "toast.html?kind=shot", SHOT_W, SHOT_H);
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
    if is_shot(window) {
        *pending_lock() = None;
    }
    let _ = window.destroy();
}

fn is_shot(window: &WebviewWindow) -> bool {
    window.url().is_ok_and(|u| u.query().unwrap_or("").contains("kind=shot"))
}

/// The toast's number (its label is "toast-<n>").
fn number(window: &WebviewWindow) -> u64 {
    window.label().strip_prefix("toast-").and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// The toast page has drawn and its content is `height` CSS pixels tall: size the window to it,
/// place it in the corner and show it on top, without taking the focus.
pub fn ready(window: &WebviewWindow, height: f64) {
    SHOWN.fetch_max(number(window), Ordering::SeqCst);
    let shot = is_shot(window);
    let w = if shot { SHOT_W } else { NOTICE_W };
    let h = if height.is_finite() && height >= 24.0 {
        height.min(400.0)
    } else if shot {
        SHOT_H
    } else {
        NOTICE_H
    };
    applog::info(&format!("toast: {} page ready ({h} pt tall), shown", window.label()));
    show_without_focus(window, w, h);
}

/// Where a `w` x `h` point toast goes on the main screen's work area, in physical pixels:
/// (x, y, width, height, scale factor). MARGIN from the edges.
/// Windows: top-right (Windows' own notifications use the bottom-right corner).
/// macOS: bottom-right, below the menu bar (its notifications use the top-right corner).
fn placement(app: &AppHandle, w: f64, h: f64) -> (i32, i32, i32, i32, f64) {
    let monitor = app.primary_monitor().ok().flatten();
    let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor());
    let px = |v: f64| (v * scale).round() as i32;
    let (pw, ph, margin) = (px(w), px(h), px(MARGIN));
    match monitor {
        Some(m) => {
            let area = m.work_area();
            let right = area.position.x + area.size.width as i32;
            let y = if cfg!(windows) {
                area.position.y + margin
            } else {
                area.position.y + area.size.height as i32 - ph - margin
            };
            (right - pw - margin, y, pw, ph, scale)
        }
        None => (px(40.0), px(40.0), pw, ph, scale),
    }
}

/// Windows: one SetWindowPos that sizes, places and shows the toast on top with SWP_NOACTIVATE
/// (Tauri's show() would activate the window and take the keyboard from the user's app), in
/// physical pixels. The window has no frame or shadow, so its outer size is its content size;
/// SWP_FRAMECHANGED makes Windows recompute the frame before the web view is sized to the client
/// area. A moment later, on the main thread, `nudge` resizes it once more so the visible web view
/// repaints, and logs where it ended up.
#[cfg(windows)]
fn show_without_focus(window: &WebviewWindow, w: f64, h: f64) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_SHOWWINDOW,
    };
    let (x, y, pw, ph, _) = placement(window.app_handle(), w, h);
    let hwnd = match window.hwnd() {
        Ok(hwnd) => hwnd,
        Err(e) => {
            applog::error(&format!("toast: no window handle for {}: {e}", window.label()));
            return;
        }
    };
    let flags = SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_FRAMECHANGED;
    if unsafe { SetWindowPos(hwnd.0 as _, HWND_TOPMOST, x, y, pw, ph, flags) } == 0 {
        applog::error(&format!("toast: could not show {}", window.label()));
        return;
    }
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        let target = window.clone();
        let _ = window.run_on_main_thread(move || nudge(&target, pw, ph));
    });
}

/// Windows: resizes the shown toast to `pw` x `ph` + 1 and back, so the web view gets a WM_SIZE
/// while visible and paints, then logs the window's final position and size.
#[cfg(windows)]
fn nudge(window: &WebviewWindow, pw: i32, ph: i32) {
    use windows_sys::Win32::{
        Foundation::{HWND, RECT},
        UI::WindowsAndMessaging::{
            GetClientRect, GetWindowRect, SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        },
    };
    // Already closed (its timer, or a newer toast): nothing to do.
    let Ok(handle) = window.hwnd() else { return };
    let hwnd: HWND = handle.0 as _;
    let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
    let mut outer = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    let ok = unsafe {
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, pw, ph + 1, flags);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, pw, ph, flags);
        GetWindowRect(hwnd, &mut outer) != 0 && GetClientRect(hwnd, &mut client) != 0
    };
    if !ok {
        return;
    }
    applog::info(&format!(
        "toast: {} at x={} y={}, window {}x{} px, client {}x{} px",
        window.label(),
        outer.left,
        outer.top,
        outer.right - outer.left,
        outer.bottom - outer.top,
        client.right - client.left,
        client.bottom - client.top
    ));
}

/// Elsewhere the toast is built visible: size it to its content and place it in the corner.
#[cfg(not(windows))]
fn show_without_focus(window: &WebviewWindow, w: f64, h: f64) {
    use tauri::{PhysicalPosition, PhysicalSize};
    let (x, y, pw, ph, _) = placement(window.app_handle(), w, h);
    let _ = window.set_size(PhysicalSize::new(pw as u32, ph as u32));
    let _ = window.set_position(PhysicalPosition::new(x, y));
    applog::info(&format!("toast: {} at x={x} y={y}, window {pw}x{ph} px", window.label()));
}

/// Opens a toast of `w` x `h` points (see `placement`), always on top, without taking the focus.
/// `ready` sizes it to its page and places it again once the page has drawn.
fn open(app: &AppHandle, url: &str, w: f64, h: f64) {
    close_all(app);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let label = format!("toast-{n}");
    let (x, y, _, _, scale) = placement(app, w, h);
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title("HL Desk")
        .inner_size(w, h)
        .position(x as f64 / scale, y as f64 / scale)
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
