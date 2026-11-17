//! Shared helper for the local SQLite store. Each feature keeps its own file
//! (for example "clipboard.db") in the app data folder, so features never share tables.

use std::error::Error;

use tauri::{AppHandle, Manager};

#[allow(dead_code)] // used by the features once they store data
pub fn open(app: &AppHandle, file: &str) -> Result<rusqlite::Connection, Box<dyn Error>> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    Ok(rusqlite::Connection::open(dir.join(file))?)
}
