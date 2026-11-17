//! Clipboard feature (Rust side). Only this file and src/features/clipboard/ belong to it.
//!
//! A background thread checks the system clipboard about twice a second and stores each new
//! text copy in clipboard.db. Unpinned items older than `keep_hours` are deleted at start-up
//! and every minute. The front end is told about changes with the "clipboard://changed" event.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{ipc::Invoke, AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

const CHANGED_EVENT: &str = "clipboard://changed";
const MAX_ITEM_BYTES: usize = 100 * 1024;
const DEFAULT_KEEP_HOURS: i64 = 24;
const LIST_LIMIT: i64 = 500;

struct Clipboard {
    db: Mutex<Connection>,
    /// Hash of text we wrote ourselves (clipboard_copy), so the watcher does not record it again.
    skip_hash: Mutex<Option<u64>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: i64,
    text: String,
    /// Unix seconds.
    created_at: i64,
    pinned: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    keep_hours: i64,
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

fn open_db(app: &AppHandle) -> Connection {
    let conn = crate::db::open(app, "clipboard.db").unwrap_or_else(|e| {
        eprintln!("clipboard: cannot open clipboard.db ({e}); history is kept in memory only");
        Connection::open_in_memory().expect("in-memory SQLite")
    });
    let setup = conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS items (
             id INTEGER PRIMARY KEY,
             text TEXT NOT NULL,
             created_at INTEGER NOT NULL,
             pinned INTEGER NOT NULL DEFAULT 0
         );
         CREATE INDEX IF NOT EXISTS items_created ON items(created_at);
         CREATE TABLE IF NOT EXISTS settings (
             key TEXT PRIMARY KEY,
             value INTEGER NOT NULL
         );",
    );
    if let Err(e) = setup {
        eprintln!("clipboard: cannot create tables: {e}");
    }
    conn
}

fn keep_hours(conn: &Connection) -> i64 {
    conn.query_row("SELECT value FROM settings WHERE key = 'keep_hours'", [], |r| r.get::<_, i64>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(DEFAULT_KEEP_HOURS)
}

/// Deletes unpinned items older than keep_hours. Returns how many were deleted.
fn delete_expired(conn: &Connection) -> usize {
    let cutoff = now() - keep_hours(conn) * 3600;
    conn.execute("DELETE FROM items WHERE pinned = 0 AND created_at < ?1", params![cutoff])
        .unwrap_or(0)
}

/// Stores a newly copied text. Returns true when the list changed.
fn record(conn: &Connection, text: &str) -> bool {
    if text.trim().is_empty() || text.len() > MAX_ITEM_BYTES {
        return false;
    }
    let latest: Option<(i64, String)> = conn
        .query_row("SELECT id, text FROM items ORDER BY created_at DESC, id DESC LIMIT 1", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .optional()
        .unwrap_or(None);
    let result = match latest {
        // Same text copied again: move it to the top instead of adding a duplicate.
        Some((id, latest_text)) if latest_text == text => {
            conn.execute("UPDATE items SET created_at = ?1 WHERE id = ?2", params![now(), id])
        }
        _ => conn.execute(
            "INSERT INTO items (text, created_at, pinned) VALUES (?1, ?2, 0)",
            params![text, now()],
        ),
    };
    result.is_ok()
}

/// Called by the watcher with the clipboard's current text.
fn on_copy(app: &AppHandle, text: &str) {
    let state = app.state::<Clipboard>();
    {
        let mut skip = state.skip_hash.lock().unwrap();
        if *skip == Some(hash(text)) {
            *skip = None;
            return;
        }
    }
    let changed = record(&state.db.lock().unwrap(), text);
    if changed {
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

/// Runs every ~500 ms on the watcher thread; every minute it also deletes expired items.
fn cleanup_if_due(app: &AppHandle, last: &mut Instant) {
    if last.elapsed() < Duration::from_secs(60) {
        return;
    }
    *last = Instant::now();
    let deleted = delete_expired(&app.state::<Clipboard>().db.lock().unwrap());
    if deleted > 0 {
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

/// macOS: watch NSPasteboard's change counter and skip copies that password managers mark
/// as concealed or transient (http://nspasteboard.org).
#[cfg(target_os = "macos")]
fn watch(app: AppHandle) {
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};

    const SKIP_TYPES: [&str; 2] = ["org.nspasteboard.ConcealedType", "org.nspasteboard.TransientType"];

    let mut last_count = NSPasteboard::generalPasteboard().changeCount();
    let mut last_cleanup = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(500));
        cleanup_if_due(&app, &mut last_cleanup);

        let text = autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let count = pasteboard.changeCount();
            if count == last_count {
                return None;
            }
            last_count = count;
            let types = pasteboard.types().map(|t| t.to_vec()).unwrap_or_default();
            if types.iter().any(|t| SKIP_TYPES.contains(&t.to_string().as_str())) {
                return None;
            }
            // SAFETY: NSPasteboardTypeString is a constant NSString exported by AppKit.
            let string_type = unsafe { NSPasteboardTypeString };
            pasteboard.stringForType(string_type).map(|s| s.to_string())
        });
        if let Some(text) = text {
            on_copy(&app, &text);
        }
    }
}

/// Other systems: read the clipboard text and compare its hash with the last one seen.
#[cfg(not(target_os = "macos"))]
fn watch(app: AppHandle) {
    let mut last_hash = app.clipboard().read_text().map(|t| hash(&t)).ok();
    let mut last_cleanup = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(500));
        cleanup_if_due(&app, &mut last_cleanup);

        let Ok(text) = app.clipboard().read_text() else { continue };
        let h = hash(&text);
        if last_hash != Some(h) {
            last_hash = Some(h);
            on_copy(&app, &text);
        }
    }
}

/// Called once at start-up, after the plugins are ready.
pub fn init(app: &AppHandle) {
    let conn = open_db(app);
    delete_expired(&conn);
    app.manage(Clipboard { db: Mutex::new(conn), skip_hash: Mutex::new(None) });

    let handle = app.clone();
    thread::spawn(move || watch(handle));
}

fn err(e: rusqlite::Error) -> String {
    e.to_string()
}

#[tauri::command]
fn clipboard_list(state: State<'_, Clipboard>, query: Option<String>) -> Result<Vec<Item>, String> {
    let conn = state.db.lock().unwrap();
    let query = query.unwrap_or_default();
    let mut stmt = conn
        .prepare(
            "SELECT id, text, created_at, pinned FROM items
             WHERE ?1 = '' OR instr(lower(text), lower(?1)) > 0
             ORDER BY pinned DESC, created_at DESC, id DESC
             LIMIT ?2",
        )
        .map_err(err)?;
    let rows = stmt
        .query_map(params![query.trim(), LIST_LIMIT], |r| {
            Ok(Item { id: r.get(0)?, text: r.get(1)?, created_at: r.get(2)?, pinned: r.get(3)? })
        })
        .map_err(err)?;
    let items = rows.collect::<Result<Vec<_>, _>>().map_err(err);
    items
}

/// Puts an item back on the clipboard (without recording it again) and hides the window,
/// so the user can paste straight into the app they came from.
#[tauri::command]
fn clipboard_copy(app: AppHandle, state: State<'_, Clipboard>, id: i64) -> Result<(), String> {
    let text: String = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT text FROM items WHERE id = ?1", params![id], |r| r.get(0))
        .map_err(err)?;
    *state.skip_hash.lock().unwrap() = Some(hash(&text));
    if let Err(e) = app.clipboard().write_text(text) {
        *state.skip_hash.lock().unwrap() = None;
        return Err(e.to_string());
    }
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.hide();
    }
    Ok(())
}

#[tauri::command]
fn clipboard_pin(state: State<'_, Clipboard>, id: i64, pinned: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("UPDATE items SET pinned = ?1 WHERE id = ?2", params![pinned, id]).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn clipboard_delete(state: State<'_, Clipboard>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM items WHERE id = ?1", params![id]).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn clipboard_clear_unpinned(state: State<'_, Clipboard>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("DELETE FROM items WHERE pinned = 0", []).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn clipboard_get_settings(state: State<'_, Clipboard>) -> Settings {
    Settings { keep_hours: keep_hours(&state.db.lock().unwrap()) }
}

#[tauri::command]
fn clipboard_set_keep_hours(state: State<'_, Clipboard>, hours: i64) -> Result<(), String> {
    if !(1..=720).contains(&hours) {
        return Err("Keep time must be between 1 and 720 hours".into());
    }
    let conn = state.db.lock().unwrap();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('keep_hours', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![hours],
    )
    .map_err(err)?;
    delete_expired(&conn);
    Ok(())
}

/// This feature's commands. Name each one "clipboard_<name>" (for example "clipboard_list")
/// so lib.rs routes it here, and list it in generate_handler! below.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        clipboard_list,
        clipboard_copy,
        clipboard_pin,
        clipboard_delete,
        clipboard_clear_unpinned,
        clipboard_get_settings,
        clipboard_set_keep_hours
    ]
}
