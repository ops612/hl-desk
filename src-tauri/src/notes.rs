//! Notes feature (Rust side). Only this file and src/features/notes/ belong to it.
//!
//! One table in notes.db. The title is always the first line of the body, set on every save,
//! so the front end only ever sends the body.

use std::error::Error;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{ipc::Invoke, AppHandle, Manager, State};

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS notes (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    title      TEXT    NOT NULL DEFAULT '',
    body       TEXT    NOT NULL DEFAULT '',
    updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS notes_updated_at ON notes (updated_at DESC);
";

/// Longest title and preview kept, in characters.
const TITLE_MAX: usize = 120;
const PREVIEW_MAX: usize = 160;

/// The open notes.db, or the reason it could not be opened (shown by every command).
struct Store(Mutex<Result<Connection, String>>);

/// A row in the list: no full body, just a one-line preview.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteSummary {
    id: i64,
    title: String,
    preview: String,
    updated_at: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Note {
    id: i64,
    title: String,
    body: String,
    updated_at: i64,
}

fn open_store(app: &AppHandle) -> Result<Connection, Box<dyn Error>> {
    let conn = crate::db::open(app, "notes.db")?;
    conn.execute_batch(SCHEMA)?;
    Ok(conn)
}

/// Called once at start-up, after the plugins are ready.
pub fn init(app: &AppHandle) {
    let conn = open_store(app).map_err(|e| format!("Could not open notes.db: {e}"));
    if let Err(e) = &conn {
        crate::applog::error(&format!("notes: {e}"));
    }
    app.manage(Store(Mutex::new(conn)));
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// First line of the body, trimmed.
fn title_of(body: &str) -> String {
    truncate(body.lines().next().unwrap_or("").trim(), TITLE_MAX)
}

/// First non-empty line after the title, trimmed.
fn preview_of(body: &str) -> String {
    let line = body.lines().skip(1).map(str::trim).find(|l| !l.is_empty());
    truncate(line.unwrap_or(""), PREVIEW_MAX)
}

/// Runs `f` with the open connection, turning every error into a message for the front end.
fn with_conn<T>(
    store: &State<'_, Store>,
    f: impl FnOnce(&Connection) -> rusqlite::Result<T>,
) -> Result<T, String> {
    let guard = store.0.lock().map_err(|_| "Notes store is unavailable.".to_string())?;
    let conn = guard.as_ref().map_err(|e| e.clone())?;
    f(conn).map_err(|e| e.to_string())
}

/// Newest first. With a query, only notes whose text contains it (case-insensitive for A-Z).
#[tauri::command]
fn notes_list(store: State<'_, Store>, query: Option<String>) -> Result<Vec<NoteSummary>, String> {
    let query = query.unwrap_or_default().trim().to_string();
    with_conn(&store, |conn| {
        let mut stmt = conn.prepare(
            "SELECT id, title, body, updated_at FROM notes
             WHERE ?1 = '' OR instr(lower(body), lower(?1)) > 0
             ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = stmt.query_map(params![query], |row| {
            let body: String = row.get(2)?;
            Ok(NoteSummary {
                id: row.get(0)?,
                title: row.get(1)?,
                preview: preview_of(&body),
                updated_at: row.get(3)?,
            })
        })?;
        let notes = rows.collect::<rusqlite::Result<Vec<_>>>();
        notes
    })
}

#[tauri::command]
fn notes_get(store: State<'_, Store>, id: i64) -> Result<Option<Note>, String> {
    with_conn(&store, |conn| {
        conn.query_row(
            "SELECT id, title, body, updated_at FROM notes WHERE id = ?1",
            params![id],
            |row| {
                Ok(Note {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    body: row.get(2)?,
                    updated_at: row.get(3)?,
                })
            },
        )
        .optional()
    })
}

/// Makes an empty note and returns it.
#[tauri::command]
fn notes_create(store: State<'_, Store>) -> Result<Note, String> {
    let updated_at = now_ms();
    with_conn(&store, |conn| {
        conn.execute(
            "INSERT INTO notes (title, body, updated_at) VALUES ('', '', ?1)",
            params![updated_at],
        )?;
        Ok(Note {
            id: conn.last_insert_rowid(),
            title: String::new(),
            body: String::new(),
            updated_at,
        })
    })
}

/// Stores the body, sets the title from its first line, and returns the new updated_at.
#[tauri::command]
fn notes_save(store: State<'_, Store>, id: i64, body: String) -> Result<i64, String> {
    let updated_at = now_ms();
    let title = title_of(&body);
    let changed = with_conn(&store, |conn| {
        conn.execute(
            "UPDATE notes SET title = ?1, body = ?2, updated_at = ?3 WHERE id = ?4",
            params![title, body, updated_at, id],
        )
    })?;
    if changed == 0 {
        return Err("This note no longer exists.".into());
    }
    Ok(updated_at)
}

#[tauri::command]
fn notes_delete(store: State<'_, Store>, id: i64) -> Result<(), String> {
    with_conn(&store, |conn| {
        conn.execute("DELETE FROM notes WHERE id = ?1", params![id]).map(|_| ())
    })
}

// ---- Voice to text: the computer's own dictation (no cloud API, no model in the app) ----
// Windows with Settings > Voice to text > Offline mode on: voice.rs records and transcribes in the app
// instead, and Win+H is never sent.
//
// Windows: Win+H opens Voice typing, which types into the focused field (the note's textarea).
// macOS: the standard startDictation: action, sent through NSApplication with the textarea focused.
// The voice typing bar takes the focus on Windows, so the popover does not hide on blur while
// dictation may be running (lib.rs asks keep_open()).

/// How long the popover stays open after the mic is clicked, unless it is stopped first.
const DICTATION_KEEP_OPEN_MS: i64 = 60_000;
/// Offline voice to text: up to 120 s of recording, then the time to write it down.
#[cfg(target_os = "windows")]
const OFFLINE_KEEP_OPEN_MS: i64 = 180_000;
/// The last dictation started was offline voice to text (so stopping it never sends Win+H, even
/// if the Settings switch changed since).
#[cfg(target_os = "windows")]
static OFFLINE_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Until when (Unix ms) the popover ignores focus loss. 0: not dictating.
static DICTATING_UNTIL: AtomicI64 = AtomicI64::new(0);

/// lib.rs: true while dictation may be running (the main window then stays open on blur).
pub(crate) fn keep_open() -> bool {
    now_ms() < DICTATING_UNTIL.load(Ordering::SeqCst)
}

/// What this computer offers. off: macOS dictation looks turned off in System Settings.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Dictation {
    available: bool,
    off: bool,
}

/// Windows 10 1709 (build 16299) is the first with dictation on Win+H.
#[cfg(target_os = "windows")]
const WIN_DICTATION_BUILD: u32 = 16299;

/// The real Windows build number (GetVersionEx is capped by the manifest; RtlGetVersion is not).
#[cfg(target_os = "windows")]
fn windows_build() -> u32 {
    #[repr(C)]
    struct OsVersionInfoW {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        csd: [u16; 128],
    }
    #[link(name = "ntdll")]
    extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfoW) -> i32;
    }
    let mut info = OsVersionInfoW {
        size: std::mem::size_of::<OsVersionInfoW>() as u32,
        major: 0,
        minor: 0,
        build: 0,
        platform: 0,
        csd: [0; 128],
    };
    // SAFETY: info is a valid OSVERSIONINFOW with its size set.
    if unsafe { RtlGetVersion(&mut info) } == 0 {
        info.build
    } else {
        0
    }
}

/// macOS: Keyboard > Dictation writes AppleDictationAutoEnable = 1 when it is on.
#[cfg(target_os = "macos")]
fn mac_dictation_on() -> bool {
    std::process::Command::new("defaults")
        .args(["read", "com.apple.HIToolbox", "AppleDictationAutoEnable"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "1")
        .unwrap_or(false)
}

#[tauri::command]
fn notes_dictation_status() -> Dictation {
    #[cfg(target_os = "windows")]
    {
        let build = windows_build();
        let available = build >= WIN_DICTATION_BUILD;
        if !available {
            crate::applog::warn(&format!(
                "notes: voice typing not available on Windows build {build}"
            ));
        }
        Dictation { available, off: false }
    }
    #[cfg(target_os = "macos")]
    {
        Dictation { available: true, off: !mac_dictation_on() }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        Dictation { available: false, off: false }
    }
}

/// Windows: presses Win+H (Voice typing). Modifiers still held from Ctrl+Shift+D are released
/// first, or Windows would see Ctrl+Shift+Win+H.
#[cfg(target_os = "windows")]
fn press_win_h() -> Result<(), String> {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP,
        VK_CONTROL, VK_LWIN, VK_MENU, VK_SHIFT,
    };
    const VK_H: u16 = 0x48;
    let key = |vk: u16, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
                time: 0,
                dwExtraInfo: 0,
            },
        },
    };
    let mut inputs = Vec::new();
    for vk in [VK_CONTROL, VK_SHIFT, VK_MENU] {
        // SAFETY: plain Win32 call with a virtual-key code.
        if unsafe { GetAsyncKeyState(vk as i32) } < 0 {
            inputs.push(key(vk, true));
        }
    }
    inputs.extend([key(VK_LWIN, false), key(VK_H, false), key(VK_H, true), key(VK_LWIN, true)]);
    // SAFETY: inputs is a valid slice of INPUT structs, and cbSize is their size.
    let sent = unsafe {
        SendInput(inputs.len() as u32, inputs.as_ptr(), std::mem::size_of::<INPUT>() as i32)
    };
    if sent as usize == inputs.len() {
        Ok(())
    } else {
        Err(format!("SendInput sent {sent} of {} keys", inputs.len()))
    }
}

/// Windows: true when HL Desk's main window is the foreground window, so keys we send reach it
/// (and never start Voice typing in another app).
#[cfg(target_os = "windows")]
fn main_window_in_front(app: &AppHandle) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let Some(ours) = app.get_webview_window("main").and_then(|w| w.hwnd().ok()) else {
        return false;
    };
    let ours: *mut std::ffi::c_void = ours.0 as _;
    // SAFETY: plain Win32 call with no arguments.
    let front = unsafe { GetForegroundWindow() };
    !ours.is_null() && front == ours
}

/// macOS: sends a dictation action (startDictation: / stopDictation:) to the first responder
/// (the focused textarea). Returns whether something took it.
#[cfg(target_os = "macos")]
fn send_mac_dictation(app: &AppHandle, action: objc2::runtime::Sel) -> Result<bool, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    app.run_on_main_thread(move || {
        let handled = objc2::MainThreadMarker::new().is_some_and(|mtm| {
            let ns_app = objc2_app_kit::NSApplication::sharedApplication(mtm);
            // SAFETY: a standard responder action with no target and no sender.
            unsafe { ns_app.sendAction_to_from(action, None, None) }
        });
        let _ = tx.send(handled);
    })
    .map_err(|e| e.to_string())?;
    rx.recv_timeout(std::time::Duration::from_secs(2))
        .map_err(|e| e.to_string())
}

/// Mic button / Ctrl+Shift+D: the page has focused the textarea at the cursor; this makes sure
/// the window has the focus, then starts the computer's dictation. Returns false on macOS when
/// nothing took the action (dictation is probably off). Windows: nothing is sent unless our
/// window is in front, so Voice typing can only type into the note.
#[tauri::command]
async fn notes_dictate(app: AppHandle) -> Result<bool, String> {
    // Offline voice to text: record from the default microphone (voice.rs); the page stops it
    // with voice_stop and inserts the text itself.
    #[cfg(target_os = "windows")]
    OFFLINE_STARTED.store(crate::voice::enabled(&app), Ordering::SeqCst);
    #[cfg(target_os = "windows")]
    if OFFLINE_STARTED.load(Ordering::SeqCst) {
        DICTATING_UNTIL.store(now_ms() + OFFLINE_KEEP_OPEN_MS, Ordering::SeqCst);
        let result = crate::voice::start(&app);
        match &result {
            Ok(()) => crate::applog::info("notes: offline voice to text started"),
            Err(e) => {
                crate::applog::error(&format!("notes: could not start offline voice to text: {e}"));
                DICTATING_UNTIL.store(0, Ordering::SeqCst);
            }
        }
        return result.map(|()| true);
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_focus();
    }
    DICTATING_UNTIL.store(now_ms() + DICTATION_KEEP_OPEN_MS, Ordering::SeqCst);
    // Let the focus settle before the keys go out.
    std::thread::sleep(std::time::Duration::from_millis(120));
    #[cfg(target_os = "windows")]
    let result = if main_window_in_front(&app) {
        press_win_h().map(|()| true)
    } else {
        Err("HL Desk is not the window in front, so dictation was not started.".into())
    };
    #[cfg(target_os = "macos")]
    let result = send_mac_dictation(&app, objc2::sel!(startDictation:));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let result: Result<bool, String> = Err("Dictation is not supported here.".into());
    match &result {
        Ok(true) => crate::applog::info("notes: dictation started"),
        Ok(false) => crate::applog::warn("notes: startDictation: was not handled (dictation off?)"),
        Err(e) => crate::applog::error(&format!("notes: could not start dictation: {e}")),
    }
    if !matches!(result, Ok(true)) {
        DICTATING_UNTIL.store(0, Ordering::SeqCst);
    }
    result
}

/// Mic clicked again, Esc, the note closed or the 60 s ran out: the popover hides on blur again.
/// end (the mic or Esc only): also ends the computer's dictation, but only one we started that
/// may still be running, so a dictation the user already closed is not opened again.
/// Async so it does not run on the main thread (macOS waits on it below).
#[tauri::command]
async fn notes_dictation_stop(app: AppHandle, end: bool) {
    let until = DICTATING_UNTIL.swap(0, Ordering::SeqCst);
    if until == 0 {
        return;
    }
    crate::applog::info("notes: dictation stopped (popover hides on blur again)");
    if !end || now_ms() >= until {
        return;
    }
    // Windows: Win+H toggles Voice typing off. Only with our window in front, so the keys can
    // never open it in another app.
    #[cfg(target_os = "windows")]
    {
        if OFFLINE_STARTED.load(Ordering::SeqCst) {
            crate::applog::info("notes: offline voice to text was started; Win+H not sent");
        } else if !main_window_in_front(&app) {
            crate::applog::warn("notes: HL Desk not in front; Voice typing left as it is");
        } else if let Err(e) = press_win_h() {
            crate::applog::error(&format!("notes: could not stop Voice typing: {e}"));
        } else {
            crate::applog::info("notes: Voice typing ended (Win+H)");
        }
    }
    #[cfg(target_os = "macos")]
    {
        match send_mac_dictation(&app, objc2::sel!(stopDictation:)) {
            Ok(true) => crate::applog::info("notes: dictation ended (stopDictation:)"),
            Ok(false) => crate::applog::warn("notes: stopDictation: was not handled"),
            Err(e) => crate::applog::error(&format!("notes: could not stop dictation: {e}")),
        }
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let _ = app;
}

/// This feature's commands. Name each one "notes_<name>" (for example "notes_list")
/// so lib.rs routes it here, and list it in generate_handler! below.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        notes_list,
        notes_get,
        notes_create,
        notes_save,
        notes_delete,
        notes_dictation_status,
        notes_dictate,
        notes_dictation_stop
    ]
}
