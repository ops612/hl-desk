//! "Mark up in HL Desk" after a device screenshot (setting: on by default on Windows, off on
//! macOS). When the computer's own screenshot tool makes a new screenshot, a toast offers to open
//! it in the editor.
//! - Windows: Win+Shift+S / Snipping Tool put the image on the clipboard. A thread checks the
//!   clipboard's sequence number twice a second (no clipboard access while nothing changes) and,
//!   when it changed and holds an image HL Desk did not put there whose owner is the snipping
//!   tool, reads it and shows the toast.
//!   Clipboard history (clipboard.rs) stays text only: images are never stored there.
//! - macOS: Cmd+Shift+3/4/5 save a file in the screenshot folder (`defaults read
//!   com.apple.screencapture location`, default Desktop). Only while the setting is on, a thread
//!   lists that folder every 1.5 s and offers each new screenshot (PNG, JPEG or HEIC) once it is
//!   fully written. The folder is never touched while the setting is off (macOS asks for Desktop
//!   access the first time; Settings explains that when the user turns it on).

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
/// macOS: the screenshot folder could not be read last time (Desktop access refused).
static FOLDER_BLOCKED: AtomicBool = AtomicBool::new(false);

/// macOS: true if the last look at the screenshot folder failed. Always false elsewhere.
pub fn folder_blocked() -> bool {
    FOLDER_BLOCKED.load(Ordering::SeqCst)
}

/// macOS: lists the screenshot folder once (macOS asks for Desktop access the first time and
/// waits for the answer). Returns true if it can be read.
#[cfg(target_os = "macos")]
pub fn check_folder() -> bool {
    let dir = mac::folder();
    let result = std::fs::read_dir(&dir);
    match &result {
        Ok(_) => applog::info(&format!("device shot: {} can be read", dir.display())),
        Err(e) => applog::warn(&format!("device shot: cannot read {}: {e}", dir.display())),
    }
    FOLDER_BLOCKED.store(result.is_err(), Ordering::SeqCst);
    result.is_ok()
}

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
        GetClipboardSequenceNumber, IsClipboardFormatAvailable, RegisterClipboardFormatW,
    };
    // CF_BITMAP, CF_DIB, CF_DIBV5.
    const IMAGE_FORMATS: [u32; 3] = [2, 8, 17];
    const CF_UNICODETEXT: u32 = 13;
    /// Windows' screenshot tools (Win+Shift+S, Print Screen, Snipping Tool).
    const SNIP_TOOLS: [&str; 2] = ["ScreenClippingHost.exe", "SnippingTool.exe"];

    let html_name: Vec<u16> = "HTML Format".encode_utf16().chain(Some(0)).collect();
    // SAFETY: a null-terminated UTF-16 name; returns the format id (0 on failure).
    let html_format = unsafe { RegisterClipboardFormatW(html_name.as_ptr()) };

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
        // Only Windows' own snipping tool counts as a device screenshot; an image copied from a
        // browser or Office is not offered. The bitmap is read only once this passes.
        // SAFETY: as above, only reads the clipboard's format list.
        let image_only = ![CF_UNICODETEXT, html_format]
            .iter()
            .any(|&f| f != 0 && unsafe { IsClipboardFormatAvailable(f) } != 0);
        match clipboard_owner_exe() {
            Some(exe) => {
                // Windows 10's Snip & Sketch (Win+Shift+S) hands the image to the clipboard
                // service, so the owner shows as svchost.exe: count that only for an image-only
                // copy (seen on a real Windows 10 PC, 2026-11-17).
                let snip = SNIP_TOOLS.iter().any(|t| exe.eq_ignore_ascii_case(t))
                    || (exe.eq_ignore_ascii_case("svchost.exe") && image_only);
                applog::info(&format!(
                    "device shot: clipboard image from {exe}, {}",
                    if snip { "qualifies" } else { "skipped (not the snipping tool)" }
                ));
                if !snip {
                    continue;
                }
            }
            None => {
                // Owner unknown: an image that came with text or HTML is a copy from an app.
                let with_text = !image_only;
                applog::info(&format!(
                    "device shot: clipboard owner unknown, {}",
                    if with_text { "skipped (image came with text or HTML)" } else { "qualifies (image only)" }
                ));
                if with_text {
                    continue;
                }
            }
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

/// The file name (for example "SnippingTool.exe") of the process that owns the clipboard, or
/// None if it cannot be told.
#[cfg(windows)]
fn clipboard_owner_exe() -> Option<String> {
    use windows_sys::Win32::{
        Foundation::CloseHandle,
        System::{
            DataExchange::GetClipboardOwner,
            Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::WindowsAndMessaging::GetWindowThreadProcessId,
    };
    // SAFETY: plain Win32 queries with valid out-pointers; the process handle is closed below.
    unsafe {
        let owner = GetClipboardOwner();
        if owner.is_null() {
            return None;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(owner, &mut pid);
        if pid == 0 {
            return None;
        }
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // 0: PROCESS_NAME_WIN32, a normal path.
        let ok = QueryFullProcessImageNameW(process, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        path.rsplit('\\').next().map(str::to_string)
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use std::path::{Path, PathBuf};

    /// The folder macOS saves screenshots to.
    pub fn folder() -> PathBuf {
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

    /// The image types macOS can save screenshots as (`defaults write com.apple.screencapture type`).
    const TYPES: [&str; 4] = ["png", "jpg", "jpeg", "heic"];

    pub fn is_png(path: &Path) -> bool {
        path.extension().is_some_and(|x| x.eq_ignore_ascii_case("png"))
    }

    pub fn images(dir: &Path) -> std::io::Result<std::collections::HashSet<PathBuf>> {
        Ok(std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| TYPES.iter().any(|t| x.eq_ignore_ascii_case(t)))
            })
            .collect())
    }

    /// A JPEG or HEIC screenshot as PNG (the editor and the shot store use PNG), made by macOS's
    /// own `sips` into a temporary file.
    pub fn to_png(path: &Path) -> Option<Vec<u8>> {
        let out = std::env::temp_dir().join(format!("hl-desk-shot-{}.png", super::super::now_ms()));
        let converted = std::process::Command::new("/usr/bin/sips")
            .args(["-s", "format", "png"])
            .arg(path)
            .arg("--out")
            .arg(&out)
            .output()
            .is_ok_and(|o| o.status.success());
        let png = if converted { std::fs::read(&out).ok() } else { None };
        let _ = std::fs::remove_file(&out);
        png
    }
}

#[cfg(target_os = "macos")]
fn watch(app: AppHandle) {
    use std::{
        collections::HashSet,
        path::{Path, PathBuf},
        time::Instant,
    };

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

    /// Waits until the file stops growing (macOS writes it after its own thumbnail goes away),
    /// then returns it as PNG.
    fn read_when_complete(path: &Path) -> Option<Vec<u8>> {
        let mut last = 0;
        for _ in 0..20 {
            let len = std::fs::metadata(path).ok()?.len();
            if len > 0 && len == last {
                return if mac::is_png(path) { std::fs::read(path).ok() } else { mac::to_png(path) };
            }
            last = len;
            thread::sleep(Duration::from_millis(300));
        }
        None
    }

    let mut known: Option<(PathBuf, HashSet<PathBuf>)> = None;
    // The folder is looked up only while the setting is on, so nothing touches it at launch.
    let mut dir: Option<PathBuf> = None;
    let mut dir_checked = Instant::now();
    let mut warned = false;
    loop {
        thread::sleep(Duration::from_millis(1500));
        if !ON.load(Ordering::SeqCst) {
            known = None;
            continue;
        }
        if dir.is_none() || dir_checked.elapsed() > Duration::from_secs(30) {
            dir = Some(mac::folder());
            dir_checked = Instant::now();
        }
        let Some(dir) = dir.clone() else { continue };
        let files = match mac::images(&dir) {
            Ok(files) => {
                FOLDER_BLOCKED.store(false, Ordering::SeqCst);
                warned = false;
                files
            }
            Err(e) => {
                FOLDER_BLOCKED.store(true, Ordering::SeqCst);
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
                None => applog::warn("device shot: the screenshot file was not complete in time, or could not be converted"),
            }
        }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
fn watch(_app: AppHandle) {}
