//! "Mark up in HL Desk" after a device screenshot (setting, on by default). When the computer's
//! own screenshot tool makes a new screenshot, a toast offers to open it in the editor.
//! - Windows: Win+Shift+S / Snipping Tool put the image on the clipboard. A thread checks the
//!   clipboard's sequence number twice a second (no clipboard access while nothing changes) and,
//!   when it changed and holds an image HL Desk did not put there, reads it and shows the toast.
//!   Clipboard history (clipboard.rs) stays text only: images are never stored there.
//! - macOS: Cmd+Shift+3/4/5 save a file in the screenshot folder (`defaults read
//!   com.apple.screencapture location`, default Desktop). A thread lists that folder every
//!   1.5 s while the setting is on and offers each new screenshot PNG once it is fully written.

use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::Duration,
};

use tauri::AppHandle;

use crate::applog;

/// The setting, cached so the watcher does not read settings.json on every tick.
static ON: AtomicBool = AtomicBool::new(true);
/// When HL Desk last wrote an image to the clipboard itself (editor Copy), Unix ms.
static OWN_WRITE_AT: AtomicU64 = AtomicU64::new(0);

pub fn init(app: &AppHandle) {
    let on = crate::settings::markup_after_shot(app);
    ON.store(on, Ordering::SeqCst);
    applog::info(&format!("device shot: watcher started (mark up offer {})", if on { "on" } else { "off" }));
    let app = app.clone();
    thread::spawn(move || watch(app));
}

pub fn set_on(on: bool) {
    ON.store(on, Ordering::SeqCst);
}

/// Called just before HL Desk puts an image on the clipboard, so it is not offered back.
pub fn mark_own_write() {
    OWN_WRITE_AT.store(super::now_ms() as u64, Ordering::SeqCst);
}

/// An image from the editor's Copy within the last 2 seconds is ours.
#[cfg_attr(not(windows), allow(dead_code))]
fn own_write_recent() -> bool {
    (super::now_ms() as u64).saturating_sub(OWN_WRITE_AT.load(Ordering::SeqCst)) < 2000
}

#[cfg(windows)]
fn watch(app: AppHandle) {
    use std::hash::{Hash, Hasher};
    use tauri_plugin_clipboard_manager::ClipboardExt;
    use windows_sys::Win32::System::DataExchange::{
        GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    };
    // CF_BITMAP, CF_DIB, CF_DIBV5.
    const IMAGE_FORMATS: [u32; 3] = [2, 8, 17];

    // SAFETY: GetClipboardSequenceNumber only reads a counter; it needs no open clipboard.
    let seq = || unsafe { GetClipboardSequenceNumber() };
    let mut last_seq = seq();
    let mut last_hash: Option<u64> = None;
    loop {
        thread::sleep(Duration::from_millis(500));
        if seq() == last_seq {
            continue;
        }
        // The writer may still be adding formats: wait until the counter stops moving.
        thread::sleep(Duration::from_millis(300));
        last_seq = seq();
        if !ON.load(Ordering::SeqCst) || super::CAPTURING.load(Ordering::SeqCst) {
            continue;
        }
        // SAFETY: IsClipboardFormatAvailable only reads the clipboard's format list.
        let has_image =
            IMAGE_FORMATS.iter().any(|&f| unsafe { IsClipboardFormatAvailable(f) } != 0);
        if !has_image {
            continue;
        }
        if own_write_recent() {
            applog::info("device shot: clipboard image is our own copy, skipped");
            continue;
        }
        let image = match app.clipboard().read_image() {
            Ok(image) => image,
            Err(e) => {
                applog::warn(&format!("device shot: could not read the clipboard image: {e}"));
                continue;
            }
        };
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        image.rgba().hash(&mut hasher);
        let hash = hasher.finish();
        if last_hash == Some(hash) {
            continue; // the same image again
        }
        last_hash = Some(hash);
        applog::info(&format!(
            "device shot: new clipboard image {}x{}",
            image.width(),
            image.height()
        ));
        match super::png_bytes(image.rgba(), image.width(), image.height()) {
            Ok(png) => super::toast::offer(&app, png),
            Err(e) => applog::error(&format!("device shot: could not encode the image: {e}")),
        }
    }
}

#[cfg(target_os = "macos")]
fn watch(app: AppHandle) {
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
        time::Instant,
    };

    /// The folder macOS saves screenshots to.
    fn folder() -> PathBuf {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let set = std::process::Command::new("/usr/bin/defaults")
            .args(["read", "com.apple.screencapture", "location"])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .filter(|s| !s.is_empty());
        let dir = match set {
            Some(s) if s == "~" => home.clone(),
            Some(s) if s.starts_with("~/") => home.join(&s[2..]),
            Some(s) => PathBuf::from(s),
            None => home.join("Desktop"),
        };
        if dir.is_dir() {
            dir
        } else {
            home.join("Desktop")
        }
    }

    fn pngs(dir: &Path) -> std::io::Result<HashSet<PathBuf>> {
        Ok(std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("png")))
            .collect())
    }

    /// macOS marks its screenshots with kMDItemIsScreenCapture; the name is a fallback.
    fn is_screenshot(path: &Path) -> bool {
        let marked = std::process::Command::new("/usr/bin/xattr")
            .args(["-p", "com.apple.metadata:kMDItemIsScreenCapture"])
            .arg(path)
            .output()
            .is_ok_and(|o| o.status.success());
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        marked || name.starts_with("Screenshot") || name.starts_with("Screen Shot")
    }

    /// Waits until the file stops growing (macOS writes it after its own thumbnail goes away).
    fn read_when_complete(path: &Path) -> Option<Vec<u8>> {
        let mut last = 0;
        for _ in 0..20 {
            let len = std::fs::metadata(path).ok()?.len();
            if len > 0 && len == last {
                return std::fs::read(path).ok();
            }
            last = len;
            thread::sleep(Duration::from_millis(300));
        }
        None
    }

    // Do not touch the folder during start-up (macOS may ask once for Desktop access).
    thread::sleep(Duration::from_secs(3));
    let mut known: Option<(PathBuf, HashSet<PathBuf>)> = None;
    let mut dir = folder();
    let mut dir_checked = Instant::now();
    let mut warned = false;
    loop {
        thread::sleep(Duration::from_millis(1500));
        if !ON.load(Ordering::SeqCst) {
            known = None;
            continue;
        }
        if dir_checked.elapsed() > Duration::from_secs(30) {
            dir = folder();
            dir_checked = Instant::now();
        }
        let files = match pngs(&dir) {
            Ok(files) => files,
            Err(e) => {
                if !warned {
                    applog::warn(&format!("device shot: cannot read {}: {e}", dir.display()));
                    warned = true;
                }
                continue;
            }
        };
        let fresh: Vec<PathBuf> = match &known {
            Some((d, seen)) if *d == dir => files.difference(seen).cloned().collect(),
            _ => {
                applog::info(&format!("device shot: watching {}", dir.display()));
                Vec::new()
            }
        };
        known = Some((dir.clone(), files));
        if super::CAPTURING.load(Ordering::SeqCst) {
            continue;
        }
        // The newest screenshot only, if several arrive at once.
        if let Some(path) = fresh.into_iter().filter(|p| is_screenshot(p)).max() {
            applog::info(&format!("device shot: new screenshot {}", path.display()));
            match read_when_complete(&path) {
                Some(png) => super::toast::offer(&app, png),
                None => applog::warn("device shot: the screenshot file was not complete in time"),
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn watch(_app: AppHandle) {}
