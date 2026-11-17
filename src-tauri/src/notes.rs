//! Notes feature (Rust side). Only this file and src/features/notes/ belong to it.
//!
//! One table in notes.db. The title is always the first line of the body, set on every save,
//! so the front end only ever sends the body.

use std::error::Error;
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
        eprintln!("{e}");
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

/// This feature's commands. Name each one "notes_<name>" (for example "notes_list")
/// so lib.rs routes it here, and list it in generate_handler! below.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![notes_list, notes_get, notes_create, notes_save, notes_delete]
}
