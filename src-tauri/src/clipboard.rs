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
use serde::{Deserialize, Serialize};
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

#[derive(Serialize, Deserialize)]
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
        crate::applog::error(&format!("clipboard: cannot open clipboard.db ({e}); history is kept in memory only"));
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
        crate::applog::error(&format!("clipboard: cannot create tables: {e}"));
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
            #[cfg(windows)]
            match is_private_copy() {
                None => continue, // clipboard busy: look again on the next tick
                Some(true) => {
                    last_hash = Some(h);
                    continue;
                }
                Some(false) => {}
            }
            last_hash = Some(h);
            on_copy(&app, &text);
        }
    }
}

/// Windows: true when the copy is marked private, the way password managers and Windows itself
/// do it: an "ExcludeClipboardContentFromMonitorProcessing" or "Clipboard Viewer Ignore" format,
/// or "CanIncludeInClipboardHistory" set to 0. None when the clipboard could not be opened.
#[cfg(windows)]
fn is_private_copy() -> Option<bool> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard,
        RegisterClipboardFormatW,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

    fn format(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
    }
    // SAFETY: IsClipboardFormatAvailable only reads the clipboard's format list.
    let present = |f: u32| f != 0 && unsafe { IsClipboardFormatAvailable(f) } != 0;

    if present(format("ExcludeClipboardContentFromMonitorProcessing"))
        || present(format("Clipboard Viewer Ignore"))
    {
        return Some(true);
    }
    let history = format("CanIncludeInClipboardHistory");
    if !present(history) {
        return Some(false);
    }
    // The value is a DWORD; 0 means "keep out of clipboard history".
    // SAFETY: the clipboard is opened (no owner window) and closed again on every path; the
    // handle from GetClipboardData is only read while locked and while the clipboard is open.
    unsafe {
        if OpenClipboard(0 as _) == 0 {
            return None;
        }
        let mut private = false;
        let data = GetClipboardData(history);
        if data as usize != 0 {
            let value = GlobalLock(data as _) as *const u32;
            if !value.is_null() {
                if GlobalSize(data as _) >= 4 {
                    private = value.read_unaligned() == 0;
                }
                GlobalUnlock(data as _);
            }
        }
        CloseClipboard();
        Some(private)
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

/// Puts an item back on the clipboard (without recording it again). The front end shows
/// "Copied" and then hides the window, so the user can paste into the app they came from.
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

fn check_hours(hours: i64) -> Result<(), String> {
    if !(1..=720).contains(&hours) {
        return Err("Keep time must be between 1 and 720 hours".into());
    }
    Ok(())
}

fn store_keep_hours(conn: &Connection, hours: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('keep_hours', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![hours],
    )
}

/// Sets the keep time and deletes the unpinned items it no longer covers. Returns those items,
/// so the front end can offer "N older items removed · Undo" (clipboard_undo_keep).
#[tauri::command]
fn clipboard_set_keep_hours(state: State<'_, Clipboard>, hours: i64) -> Result<Vec<Item>, String> {
    check_hours(hours)?;
    let conn = state.db.lock().unwrap();
    let before = keep_hours(&conn);
    let tx = conn.unchecked_transaction().map_err(err)?;
    store_keep_hours(&tx, hours).map_err(err)?;
    let cutoff = now() - hours * 3600;
    let removed = {
        let mut stmt = tx
            .prepare(
                "SELECT id, text, created_at, pinned FROM items
                 WHERE pinned = 0 AND created_at < ?1 ORDER BY created_at, id",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![cutoff], |r| {
                Ok(Item { id: r.get(0)?, text: r.get(1)?, created_at: r.get(2)?, pinned: r.get(3)? })
            })
            .map_err(err)?;
        let removed = rows.collect::<Result<Vec<_>, _>>().map_err(err)?;
        removed
    };
    tx.execute("DELETE FROM items WHERE pinned = 0 AND created_at < ?1", params![cutoff])
        .map_err(err)?;
    tx.commit().map_err(err)?;
    crate::applog::info(&format!(
        "clipboard: keep changed from {before} to {hours} hours, {} older item(s) removed",
        removed.len()
    ));
    Ok(removed)
}

/// Undo for clipboard_set_keep_hours: puts the old keep time back first (so the minute cleanup
/// cannot remove the items again), then the removed items with their own id, time and pin
/// (a new id only if a newer copy has taken that id since).
#[tauri::command]
fn clipboard_undo_keep(state: State<'_, Clipboard>, hours: i64, items: Vec<Item>) -> Result<(), String> {
    let result = undo_keep(&state, hours, &items);
    match &result {
        Ok(restored) => crate::applog::info(&format!(
            "clipboard: keep undo: back to {hours} hours, {restored} of {} item(s) restored",
            items.len()
        )),
        Err(e) => crate::applog::error(&format!("clipboard: keep undo failed: {e}")),
    }
    result.map(|_| ())
}

/// Returns how many items were put back.
fn undo_keep(state: &Clipboard, hours: i64, items: &[Item]) -> Result<usize, String> {
    check_hours(hours)?;
    let conn = state.db.lock().unwrap();
    let tx = conn.unchecked_transaction().map_err(err)?;
    store_keep_hours(&tx, hours).map_err(err)?;
    let mut restored = 0;
    for item in items.iter().filter(|i| !i.text.trim().is_empty() && i.text.len() <= MAX_ITEM_BYTES) {
        let same_id = tx
            .execute(
                "INSERT OR IGNORE INTO items (id, text, created_at, pinned) VALUES (?1, ?2, ?3, ?4)",
                params![item.id, item.text, item.created_at, item.pinned],
            )
            .map_err(err)?;
        if same_id == 0 {
            tx.execute(
                "INSERT INTO items (text, created_at, pinned) VALUES (?1, ?2, ?3)",
                params![item.text, item.created_at, item.pinned],
            )
            .map_err(err)?;
        }
        restored += 1;
    }
    tx.commit().map_err(err)?;
    Ok(restored)
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
        clipboard_set_keep_hours,
        clipboard_undo_keep
    ]
}
