//! Clipboard feature (Rust side). Only this file and src/features/clipboard/ belong to it.
//!
//! A background thread watches the system clipboard about twice a second and stores each new
//! copy in clipboard.db, as one of three kinds:
//! - "text": the text itself (up to 100 KB).
//! - "image": a PNG of the image plus a 256 px thumbnail in `<app data>/clipboard-images/`
//!   (`<stamp>.png`, `<stamp>-thumb.png`, a name that never repeats); images over 25 MB as PNG
//!   or 50 megapixels are skipped, and past 500 MB in all the oldest unpinned images go.
//! - "files": the absolute paths of copied files (Explorer, Finder), never the files themselves.
//!   The names are also kept in `text`, so search finds them.
//! When a copy holds several kinds, files win over text and text over an image (an Excel range
//! or a Word selection stays text, as before). Copies that password managers mark as private are
//! skipped, and so is HL Desk's own write when it puts an item back (clipboard_copy).
//! Unpinned items older than `keep_hours` are deleted at start-up and every minute, with their
//! image files. The front end is told about changes with the "clipboard://changed" event.
//! Every record, skip and copy-back is logged (kind and size only, never the content).

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, types::ToSql, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use tauri::{
    image::Image,
    ipc::{Invoke, Response},
    AppHandle, Emitter, Manager, State,
};
use tauri_plugin_clipboard_manager::ClipboardExt;
use xcap::image::{
    codecs::png::{CompressionType, FilterType, PngEncoder},
    imageops, ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage,
};

use crate::applog;

const CHANGED_EVENT: &str = "clipboard://changed";
const MAX_ITEM_BYTES: usize = 100 * 1024;
/// Images larger than this as PNG are not kept.
const MAX_IMAGE_BYTES: usize = 25 * 1024 * 1024;
/// Images with more pixels than this (about 200 MB of RGBA) are skipped without encoding.
const MAX_IMAGE_PIXELS: u64 = 50_000_000;
/// Most disk space clipboard-images may use; past it the oldest unpinned images are deleted.
const MAX_IMAGES_DIR_BYTES: u64 = 500 * 1024 * 1024;
/// Longest side of an image's thumbnail.
const THUMB_SIZE: u32 = 256;
const IMAGES_DIR: &str = "clipboard-images";
const DEFAULT_KEEP_HOURS: i64 = 24;
const LIST_LIMIT: i64 = 500;
/// Every column the front end sees, in the order item_from_row reads them.
const COLUMNS: &str = "id, kind, text, image_path, thumb_path, width, height, files_json, created_at, pinned";

struct Clipboard {
    db: Mutex<Connection>,
    /// Hash of text we wrote ourselves (clipboard_copy), so the watcher does not record it again.
    skip_hash: Mutex<Option<u64>>,
    /// The clipboard change counter (Windows sequence number, macOS changeCount) right after
    /// HL Desk wrote an item back itself, so the watcher skips that change whatever its kind.
    own_change: Mutex<Option<i64>>,
    /// `<app data>/clipboard-images`.
    images: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: i64,
    /// "text", "image" or "files".
    #[serde(default = "text_kind")]
    kind: String,
    /// The text; for files, their names one per line; empty for images.
    #[serde(default)]
    text: String,
    #[serde(default)]
    image_path: Option<String>,
    #[serde(default)]
    thumb_path: Option<String>,
    #[serde(default)]
    width: Option<i64>,
    #[serde(default)]
    height: Option<i64>,
    /// Files: absolute paths.
    #[serde(default)]
    files: Vec<String>,
    /// Files: how many of them no longer exist (worked out when the list is read).
    #[serde(default)]
    missing: usize,
    /// Unix seconds.
    created_at: i64,
    pinned: bool,
}

fn text_kind() -> String {
    "text".into()
}

/// What the watcher found on the clipboard.
enum Copied {
    Text(String),
    Image(Image<'static>),
    Files(Vec<String>),
}

/// The clipboard plugin's image borrows the app handle; the watcher keeps its own copy.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn owned(image: &Image<'_>) -> Image<'static> {
    Image::new_owned(image.rgba().to_vec(), image.width(), image.height())
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

fn item_from_row(r: &Row) -> rusqlite::Result<Item> {
    let files_json: Option<String> = r.get(7)?;
    let files = files_json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
    Ok(Item {
        id: r.get(0)?,
        kind: r.get(1)?,
        text: r.get(2)?,
        image_path: r.get(3)?,
        thumb_path: r.get(4)?,
        width: r.get(5)?,
        height: r.get(6)?,
        files,
        missing: 0,
        created_at: r.get(8)?,
        pinned: r.get(9)?,
    })
}

fn open_db(app: &AppHandle) -> Connection {
    let conn = crate::db::open(app, "clipboard.db").unwrap_or_else(|e| {
        applog::error(&format!("clipboard: cannot open clipboard.db ({e}); history is kept in memory only"));
        Connection::open_in_memory().expect("in-memory SQLite")
    });
    create_tables(&conn);
    conn
}

fn create_tables(conn: &Connection) {
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
        applog::error(&format!("clipboard: cannot create tables: {e}"));
    }
    if let Err(e) = migrate(conn) {
        applog::error(&format!("clipboard: cannot add the image and file columns: {e}"));
    }
}

/// 0.1.6: images and files. Adds the new columns to an existing table (old rows become "text").
fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    const ADD: [(&str, &str); 7] = [
        ("kind", "TEXT NOT NULL DEFAULT 'text'"),
        ("image_path", "TEXT"),
        ("thumb_path", "TEXT"),
        ("width", "INTEGER"),
        ("height", "INTEGER"),
        ("files_json", "TEXT"),
        // Hash of an image's pixels or of a file list, to skip the same copy twice in a row.
        ("content_hash", "INTEGER"),
    ];
    let mut stmt = conn.prepare("PRAGMA table_info(items)")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
    let existing = rows.collect::<Result<Vec<_>, _>>()?;
    for (name, decl) in ADD {
        if !existing.iter().any(|c| c == name) {
            conn.execute_batch(&format!("ALTER TABLE items ADD COLUMN {name} {decl}"))?;
            applog::info(&format!("clipboard: added column {name}"));
        }
    }
    Ok(())
}

fn keep_hours(conn: &Connection) -> i64 {
    conn.query_row("SELECT value FROM settings WHERE key = 'keep_hours'", [], |r| r.get::<_, i64>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or(DEFAULT_KEEP_HOURS)
}

/// Deletes an image's PNG and thumbnail files, but only files directly inside clipboard-images
/// that no row uses any more (an undone delete or a restored id keeps them). Returns how many.
fn remove_image_files(conn: &Connection, images: &Path, paths: &[Option<String>]) -> usize {
    let mut removed = 0;
    for p in paths.iter().flatten() {
        let path = Path::new(p);
        if path.parent() != Some(images) {
            continue;
        }
        let used: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM items WHERE image_path = ?1 OR thumb_path = ?1)",
                params![p],
                |r| r.get(0),
            )
            .unwrap_or(true);
        if used {
            continue;
        }
        match fs::remove_file(path) {
            Ok(()) => removed += 1,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => applog::warn(&format!("clipboard: cannot delete {}: {e}", path.display())),
        }
    }
    removed
}

/// Deletes the rows that match `where_sql`, then their image files. Returns how many rows.
fn delete_where(conn: &Connection, images: &Path, where_sql: &str, args: &[&dyn ToSql]) -> rusqlite::Result<usize> {
    let paths = {
        let mut stmt = conn.prepare(&format!(
            "SELECT image_path, thumb_path FROM items WHERE kind = 'image' AND ({where_sql})"
        ))?;
        let rows = stmt.query_map(args, |r| Ok([r.get::<_, Option<String>>(0)?, r.get(1)?]))?;
        let paths = rows.collect::<Result<Vec<_>, _>>()?;
        paths
    };
    let deleted = conn.execute(&format!("DELETE FROM items WHERE {where_sql}"), args)?;
    remove_image_files(conn, images, &paths.concat());
    Ok(deleted)
}

/// Deletes unpinned items older than keep_hours, with their image files. Returns how many.
fn delete_expired(conn: &Connection, images: &Path) -> usize {
    let cutoff = now() - keep_hours(conn) * 3600;
    delete_where(conn, images, "pinned = 0 AND created_at < ?1", params![cutoff]).unwrap_or(0)
}

/// True when the connection is a database file, not the in-memory fallback (which has no rows,
/// so every image would look unused).
fn is_file_db(conn: &Connection) -> bool {
    conn.query_row("SELECT file FROM pragma_database_list WHERE name = 'main'", [], |r| {
        r.get::<_, Option<String>>(0)
    })
    .ok()
    .flatten()
    .is_some_and(|f| !f.is_empty())
}

/// Image files no row uses: left over when the app quit during a Keep change's undo time.
fn remove_orphan_images(conn: &Connection, images: &Path) {
    if !is_file_db(conn) {
        applog::info("clipboard: history is in memory, unused image files are left alone");
        return;
    }
    let Ok(entries) = fs::read_dir(images) else { return };
    let paths: Vec<Option<String>> =
        entries.filter_map(Result::ok).map(|e| Some(e.path().to_string_lossy().into_owned())).collect();
    let removed = remove_image_files(conn, images, &paths);
    if removed > 0 {
        applog::info(&format!("clipboard: removed {removed} unused image file(s)"));
    }
}

fn file_size(path: &Option<String>) -> u64 {
    path.as_deref().and_then(|p| fs::metadata(p).ok()).map_or(0, |m| m.len())
}

/// Deletes the oldest unpinned images until the image files the rows own (picture and
/// thumbnail) add up to at most `cap` bytes. Never deletes `keep_id`, the image just recorded.
/// Returns how many images were deleted.
fn enforce_images_cap(conn: &Connection, images: &Path, cap: u64, keep_id: i64) -> usize {
    // (id, pinned, bytes), oldest first.
    let rows: Vec<(i64, bool, u64)> = {
        let Ok(mut stmt) = conn.prepare(
            "SELECT id, pinned, image_path, thumb_path FROM items WHERE kind = 'image'
             ORDER BY created_at, id",
        ) else {
            return 0;
        };
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, bool>(1)?, file_size(&r.get(2)?) + file_size(&r.get(3)?)))
        });
        let rows: Vec<(i64, bool, u64)> = match rows {
            Ok(rows) => rows.filter_map(Result::ok).collect(),
            Err(_) => return 0,
        };
        rows
    };
    let mut total: u64 = rows.iter().map(|&(_, _, size)| size).sum();
    let mut deleted = 0;
    for (id, pinned, size) in rows {
        if total <= cap {
            break;
        }
        if pinned || id == keep_id {
            continue;
        }
        if delete_where(conn, images, "id = ?1", params![id]).unwrap_or(0) > 0 {
            total = total.saturating_sub(size);
            deleted += 1;
        }
    }
    if deleted > 0 {
        applog::info(&format!(
            "clipboard: images over {} MB, deleted the {deleted} oldest unpinned",
            cap / (1024 * 1024)
        ));
    }
    deleted
}

/// The newest row: (id, kind, text, content_hash).
fn latest(conn: &Connection) -> Option<(i64, String, String, Option<i64>)> {
    conn.query_row(
        "SELECT id, kind, text, content_hash FROM items ORDER BY created_at DESC, id DESC LIMIT 1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )
    .optional()
    .unwrap_or(None)
}

fn move_to_top(conn: &Connection, id: i64) -> bool {
    conn.execute("UPDATE items SET created_at = ?1 WHERE id = ?2", params![now(), id]).is_ok()
}

/// Stores a newly copied text. Returns true when the list changed.
fn record_text(conn: &Connection, text: &str) -> bool {
    if text.trim().is_empty() {
        return false;
    }
    if text.len() > MAX_ITEM_BYTES {
        applog::info(&format!("clipboard: text skipped ({} KB, over 100 KB)", text.len() / 1024));
        return false;
    }
    match latest(conn) {
        // Same text copied again: move it to the top instead of adding a duplicate.
        Some((id, kind, latest_text, _)) if kind == "text" && latest_text == text => {
            applog::info("clipboard: same text again, moved to the top");
            move_to_top(conn, id)
        }
        _ => {
            let ok = conn
                .execute(
                    "INSERT INTO items (kind, text, created_at, pinned) VALUES ('text', ?1, ?2, 0)",
                    params![text, now()],
                )
                .is_ok();
            if ok {
                applog::info(&format!("clipboard: recorded text ({} chars)", text.chars().count()));
            }
            ok
        }
    }
}

fn file_name(path: &str) -> &str {
    path.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().unwrap_or(path)
}

/// A UNC path (\\server\share\..., also \\?\UNC\...) or, on Windows, a path on a mapped network
/// drive: looking it up can hang for a long time when the server is offline.
fn is_network_path(path: &str) -> bool {
    let p = path.replace('/', "\\");
    let local = match p.strip_prefix(r"\\?\").or_else(|| p.strip_prefix(r"\\.\")) {
        Some(rest) if rest.get(..4).is_some_and(|s| s.eq_ignore_ascii_case(r"UNC\")) => return true,
        Some(rest) => rest,
        None if p.starts_with(r"\\") => return true,
        None => p.as_str(),
    };
    drive_root(local).is_some_and(|root| is_remote_drive(&root))
}

/// The drive root of a Windows path ("Z:\" for "Z:\folder\a.txt").
fn drive_root(path: &str) -> Option<String> {
    match path.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => Some(format!("{}:\\", *letter as char)),
        _ => None,
    }
}

/// Whether a drive root is a mapped network drive (asks Windows only, not the server).
#[cfg(windows)]
fn is_remote_drive(root: &str) -> bool {
    const DRIVE_REMOTE: u32 = 4;
    let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) == DRIVE_REMOTE }
}

#[cfg(not(windows))]
fn is_remote_drive(_root: &str) -> bool {
    false
}

/// Whether a copied file is still there. Network paths are not looked up (counted as there),
/// so an offline share cannot freeze the list or a copy-back.
fn file_present(path: &str) -> bool {
    is_network_path(path) || Path::new(path).exists()
}

/// Stores a newly copied list of files (paths only). Returns true when the list changed.
fn record_files(conn: &Connection, paths: &[String]) -> bool {
    if paths.is_empty() {
        return false;
    }
    let json = serde_json::to_string(paths).unwrap_or_default();
    let content_hash = hash(&json) as i64;
    match latest(conn) {
        Some((id, kind, _, h)) if kind == "files" && h == Some(content_hash) => {
            applog::info("clipboard: same files again, moved to the top");
            move_to_top(conn, id)
        }
        _ => {
            let names = paths.iter().map(|p| file_name(p)).collect::<Vec<_>>().join("\n");
            let ok = conn
                .execute(
                    "INSERT INTO items (kind, text, files_json, content_hash, created_at, pinned)
                     VALUES ('files', ?1, ?2, ?3, ?4, 0)",
                    params![names, json, content_hash, now()],
                )
                .is_ok();
            if ok {
                applog::info(&format!("clipboard: recorded {} file(s)", paths.len()));
            }
            ok
        }
    }
}

/// A file name stem that never repeats (row ids can come back after a delete): the time in
/// nanoseconds plus a random number.
fn new_image_stem() -> String {
    use std::hash::BuildHasher;
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let random = std::collections::hash_map::RandomState::new().hash_one(nanos);
    format!("{nanos:x}-{random:016x}")
}

fn too_many_pixels(width: u32, height: u32) -> bool {
    width as u64 * height as u64 > MAX_IMAGE_PIXELS
}

/// Width and height from a PNG header (IHDR), without decoding the image.
#[cfg_attr(not(any(windows, target_os = "macos", test)), allow(dead_code))]
fn png_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((be(16), be(20)))
}

/// Width and height from a TIFF's first directory (tags 256 and 257), without decoding it.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn tiff_dims(b: &[u8]) -> Option<(u32, u32)> {
    let le = match b.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u32> {
        let x = [*b.get(i)?, *b.get(i + 1)?];
        let v = if le { u16::from_le_bytes(x) } else { u16::from_be_bytes(x) };
        Some(v as u32)
    };
    let u32_at = |i: usize| -> Option<u32> {
        let x = [*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?];
        Some(if le { u32::from_le_bytes(x) } else { u32::from_be_bytes(x) })
    };
    let ifd = u32_at(4)? as usize;
    let (mut width, mut height) = (None, None);
    for n in 0..u16_at(ifd)? as usize {
        let e = ifd + 2 + n * 12;
        let value = match u16_at(e + 2)? {
            3 => u16_at(e + 8)?, // SHORT
            4 => u32_at(e + 8)?, // LONG
            _ => continue,
        };
        match u16_at(e)? {
            256 => width = Some(value),
            257 => height = Some(value),
            _ => {}
        }
    }
    Some((width?, height?))
}

/// Width and height from a DIB header (BITMAPCOREHEADER, or BITMAPINFOHEADER and later).
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn dib_dims(b: &[u8]) -> Option<(u32, u32)> {
    let size = u32::from_le_bytes(b.get(..4)?.try_into().ok()?);
    if size == 12 {
        let w = u16::from_le_bytes(b.get(4..6)?.try_into().ok()?);
        let h = u16::from_le_bytes(b.get(6..8)?.try_into().ok()?);
        return Some((w as u32, h as u32));
    }
    if size < 40 {
        return None;
    }
    let w = i32::from_le_bytes(b.get(4..8)?.try_into().ok()?);
    let h = i32::from_le_bytes(b.get(8..12)?.try_into().ok()?);
    Some((w.unsigned_abs(), h.unsigned_abs())) // a negative height means top-down rows
}

/// PNG bytes of an RGBA image (fast compression).
fn png_bytes(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(rgba, width, height, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// PNG of the image scaled so its longest side is at most THUMB_SIZE.
fn thumbnail_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    if width.max(height) <= THUMB_SIZE {
        return png_bytes(rgba, width, height);
    }
    let image = RgbaImage::from_raw(width, height, rgba.to_vec()).ok_or("image size does not match")?;
    let (w, h) = if width >= height {
        (THUMB_SIZE, ((height as u64 * THUMB_SIZE as u64) / width as u64).max(1) as u32)
    } else {
        (((width as u64 * THUMB_SIZE as u64) / height as u64).max(1) as u32, THUMB_SIZE)
    };
    let thumb = imageops::thumbnail(&image, w, h);
    png_bytes(thumb.as_raw(), w, h)
}

/// Stores a newly copied image as PNG plus thumbnail. Returns true when the list changed.
fn record_image(state: &Clipboard, image: &Image<'_>) -> bool {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || image.rgba().len() != width as usize * height as usize * 4 {
        return false;
    }
    if too_many_pixels(width, height) {
        applog::info(&format!("clipboard: image skipped ({width}x{height}, too large)"));
        return false;
    }
    let mut hasher = DefaultHasher::new();
    (width, height).hash(&mut hasher);
    image.rgba().hash(&mut hasher);
    let content_hash = hasher.finish() as i64;
    {
        let conn = state.db.lock().unwrap();
        if let Some((id, kind, _, h)) = latest(&conn) {
            if kind == "image" && h == Some(content_hash) {
                applog::info("clipboard: same image again, moved to the top");
                return move_to_top(&conn, id);
            }
        }
    }
    // Encoding runs without the database lock, so the list stays responsive.
    let png = match png_bytes(image.rgba(), width, height) {
        Ok(png) => png,
        Err(e) => {
            applog::error(&format!("clipboard: could not encode the image: {e}"));
            return false;
        }
    };
    if png.len() > MAX_IMAGE_BYTES {
        applog::info(&format!(
            "clipboard: image skipped ({width}x{height}, {} MB as PNG, over 25 MB)",
            png.len() / (1024 * 1024)
        ));
        return false;
    }
    let thumb = match thumbnail_png(image.rgba(), width, height) {
        Ok(thumb) => thumb,
        Err(e) => {
            applog::error(&format!("clipboard: could not make the thumbnail: {e}"));
            return false;
        }
    };
    if let Err(e) = fs::create_dir_all(&state.images) {
        applog::error(&format!("clipboard: cannot create {}: {e}", state.images.display()));
        return false;
    }
    let conn = state.db.lock().unwrap();
    if let Err(e) = conn.execute(
        "INSERT INTO items (kind, text, width, height, content_hash, created_at, pinned)
         VALUES ('image', '', ?1, ?2, ?3, ?4, 0)",
        params![width, height, content_hash, now()],
    ) {
        applog::error(&format!("clipboard: could not store the image: {e}"));
        return false;
    }
    let id = conn.last_insert_rowid();
    let stem = new_image_stem();
    let image_path = state.images.join(format!("{stem}.png"));
    let thumb_path = state.images.join(format!("{stem}-thumb.png"));
    let written = fs::write(&image_path, &png)
        .and_then(|_| fs::write(&thumb_path, &thumb))
        .map_err(|e| e.to_string())
        .and_then(|_| {
            conn.execute(
                "UPDATE items SET image_path = ?1, thumb_path = ?2 WHERE id = ?3",
                params![image_path.to_string_lossy(), thumb_path.to_string_lossy(), id],
            )
            .map_err(|e| e.to_string())
        });
    if let Err(e) = written {
        applog::error(&format!("clipboard: could not save the image files: {e}"));
        let _ = conn.execute("DELETE FROM items WHERE id = ?1", params![id]);
        let _ = fs::remove_file(&image_path);
        let _ = fs::remove_file(&thumb_path);
        return false;
    }
    applog::info(&format!("clipboard: recorded image {width}x{height} ({} KB)", png.len() / 1024));
    enforce_images_cap(&conn, &state.images, MAX_IMAGES_DIR_BYTES, id);
    true
}

/// Called by the watcher with a new copy.
fn on_copy(app: &AppHandle, copied: Copied) {
    let state = app.state::<Clipboard>();
    let changed = match copied {
        Copied::Text(text) => {
            {
                let mut skip = state.skip_hash.lock().unwrap();
                if *skip == Some(hash(&text)) {
                    *skip = None;
                    applog::info("clipboard: skipped our own copy");
                    return;
                }
            }
            record_text(&state.db.lock().unwrap(), &text)
        }
        Copied::Image(image) => record_image(&state, &image),
        Copied::Files(paths) => record_files(&state.db.lock().unwrap(), &paths),
    };
    if changed {
        let _ = app.emit(CHANGED_EVENT, ());
    }
}

/// Reads the change counter while holding the "own write" marker (so a write in progress in
/// clipboard_copy finishes first) and tells whether this change is HL Desk's own write.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
fn own_change_check(state: &Clipboard, counter: impl FnOnce() -> i64) -> (i64, bool) {
    let mut own = state.own_change.lock().unwrap();
    let count = counter();
    if *own == Some(count) {
        *own = None;
        *state.skip_hash.lock().unwrap() = None;
        return (count, true);
    }
    (count, false)
}

/// Runs every ~500 ms on the watcher thread; every minute it also deletes expired items.
fn cleanup_if_due(app: &AppHandle, last: &mut Instant) {
    if last.elapsed() < Duration::from_secs(60) {
        return;
    }
    *last = Instant::now();
    let state = app.state::<Clipboard>();
    let deleted = delete_expired(&state.db.lock().unwrap(), &state.images);
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
    const IMAGE_TYPES: [&str; 2] = ["public.png", "public.tiff"];

    fn types_of(pasteboard: &NSPasteboard) -> Vec<String> {
        pasteboard.types().map(|t| t.to_vec().iter().map(|s| s.to_string()).collect()).unwrap_or_default()
    }

    fn is_private(types: &[String]) -> bool {
        types.iter().any(|t| SKIP_TYPES.contains(&t.as_str()))
    }

    fn read_copy(app: &AppHandle, pasteboard: &NSPasteboard, types: &[String]) -> Option<Copied> {
        if types.iter().any(|t| t == mac::FILE_URL) {
            let files = mac::read_files(pasteboard);
            if !files.is_empty() {
                return Some(Copied::Files(files));
            }
        }
        // SAFETY: NSPasteboardTypeString is a constant NSString exported by AppKit.
        let string_type = unsafe { NSPasteboardTypeString };
        if let Some(text) = pasteboard.stringForType(string_type) {
            return Some(Copied::Text(text.to_string()));
        }
        if types.iter().any(|t| IMAGE_TYPES.contains(&t.as_str())) {
            // Checked from the PNG or TIFF header first, so a huge image is never decoded.
            if let Some((w, h)) = mac::image_dims(pasteboard, &IMAGE_TYPES).filter(|&(w, h)| too_many_pixels(w, h)) {
                applog::info(&format!("clipboard: image skipped ({w}x{h}, too large)"));
                return None;
            }
            return match app.clipboard().read_image() {
                Ok(image) => Some(Copied::Image(owned(&image))),
                Err(e) => {
                    applog::warn(&format!("clipboard: could not read the copied image: {e}"));
                    None
                }
            };
        }
        None
    }

    let mut last_count = NSPasteboard::generalPasteboard().changeCount() as i64;
    let mut last_cleanup = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(500));
        cleanup_if_due(&app, &mut last_cleanup);

        let state = app.state::<Clipboard>();
        let copied = autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            if pasteboard.changeCount() as i64 == last_count {
                return None;
            }
            let (count, own) = own_change_check(&state, || pasteboard.changeCount() as i64);
            // How many copies happened since the last look (2 or more: a quick Cmd+C twice).
            let jump = count - last_count;
            last_count = count;
            if own {
                applog::info("clipboard: skipped our own copy");
                return None;
            }
            let types = types_of(&pasteboard);
            if is_private(&types) {
                applog::info("clipboard: skipped a private copy");
                return None;
            }
            let copied = read_copy(&app, &pasteboard, &types)?;
            // The private marker and the content must belong to the same copy: a password
            // manager may still have been writing, so look again before anything is stored.
            if pasteboard.changeCount() as i64 != count {
                applog::info("clipboard: the copy changed while it was read, reading it again");
                return None; // last_count is behind now, so the next tick reads the new copy
            }
            if is_private(&types_of(&pasteboard)) {
                applog::info("clipboard: skipped a private copy");
                return None;
            }
            Some((copied, jump))
        });
        if let Some((copied, jump)) = copied {
            // Translate card (translate.rs): Cmd+C twice opens it, any other copy hides it.
            match &copied {
                Copied::Text(text) => crate::translate::on_text_copied(&app, text, jump),
                _ => crate::translate::on_other_copied(&app),
            }
            on_copy(&app, copied);
        }
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use objc2::{rc::Retained, runtime::ProtocolObject};
    use objc2_app_kit::{NSPasteboard, NSPasteboardItem, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSString, NSURL};

    pub const FILE_URL: &str = "public.file-url";

    /// The paths of the copied files (one pasteboard item each).
    pub fn read_files(pasteboard: &NSPasteboard) -> Vec<String> {
        let file_type = NSString::from_str(FILE_URL);
        let Some(items) = pasteboard.pasteboardItems() else { return Vec::new() };
        items
            .to_vec()
            .iter()
            .filter_map(|item| {
                let url_string = item.stringForType(&file_type)?;
                let url = NSURL::URLWithString(&url_string)?;
                // Finder hands over file reference URLs (file:///.file/id=...): make them paths.
                let url = url.filePathURL().unwrap_or(url);
                url.path().map(|p| p.to_string())
            })
            .collect()
    }

    /// Puts the files on the pasteboard as file URLs (what Finder does), so they paste into
    /// Finder or a chat. Returns the change count after the write.
    pub fn write_files(paths: &[String]) -> Result<i64, String> {
        objc2::rc::autoreleasepool(|_| {
            let file_type = NSString::from_str(FILE_URL);
            let items: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = paths
                .iter()
                .filter_map(|p| {
                    let url = NSURL::fileURLWithPath(&NSString::from_str(p)).absoluteString()?;
                    let item = NSPasteboardItem::new();
                    item.setString_forType(&url, &file_type);
                    Some(ProtocolObject::from_retained(item))
                })
                .collect();
            let pasteboard = NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            if !pasteboard.writeObjects(&NSArray::from_retained_slice(&items)) {
                return Err("macOS did not accept the files".to_string());
            }
            Ok(pasteboard.changeCount() as i64)
        })
    }

    pub fn change_count() -> i64 {
        objc2::rc::autoreleasepool(|_| NSPasteboard::generalPasteboard().changeCount() as i64)
    }

    /// Width and height of the copied PNG or TIFF, read from its header (not decoded).
    pub fn image_dims(pasteboard: &NSPasteboard, types: &[&str]) -> Option<(u32, u32)> {
        types.iter().find_map(|t| {
            let data = pasteboard.dataForType(&NSString::from_str(t))?;
            // SAFETY: the NSData is immutable and kept alive by `data` while the slice is used.
            let bytes = unsafe { data.as_bytes_unchecked() };
            let dims = super::png_dims(bytes).or_else(|| super::tiff_dims(bytes));
            dims
        })
    }
}

/// Windows: watch the clipboard sequence number (no clipboard access while nothing changes).
#[cfg(windows)]
fn watch(app: AppHandle) {
    let mut last_seq = win::seq();
    let mut last_cleanup = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(500));
        cleanup_if_due(&app, &mut last_cleanup);
        if win::seq() == last_seq {
            continue;
        }
        // The writer may still be adding formats: give it a moment.
        thread::sleep(Duration::from_millis(300));
        let state = app.state::<Clipboard>();
        let (seq, own) = own_change_check(&state, win::seq);
        if own {
            last_seq = seq;
            applog::info("clipboard: skipped our own copy");
            continue;
        }
        match win::is_private_copy() {
            None => continue, // clipboard busy: look again on the next tick
            Some(true) => {
                last_seq = seq;
                applog::info("clipboard: skipped a private copy");
                continue;
            }
            Some(false) => {}
        }
        let copied = if win::has(win::CF_HDROP) {
            match win::read_files() {
                Some(files) => Copied::Files(files),
                None => continue,
            }
        } else if win::has(win::CF_UNICODETEXT) {
            match app.clipboard().read_text() {
                Ok(text) => Copied::Text(text),
                Err(_) => continue,
            }
        } else if win::has_image() {
            last_seq = seq;
            // Checked from the PNG or DIB header first, so a huge image is never decoded.
            if let Some((w, h)) = win::image_dims().filter(|&(w, h)| too_many_pixels(w, h)) {
                applog::info(&format!("clipboard: image skipped ({w}x{h}, too large)"));
                continue;
            }
            match app.clipboard().read_image() {
                Ok(image) => Copied::Image(owned(&image)),
                Err(e) => {
                    applog::warn(&format!("clipboard: could not read the copied image: {e}"));
                    continue;
                }
            }
        } else {
            last_seq = seq;
            continue;
        };
        // The private marker and the content must belong to the same copy: a password manager
        // may still have been writing when the marker was checked, so look again before storing.
        if win::seq() != seq {
            applog::info("clipboard: the copy changed while it was read, reading it again");
            last_seq = -1; // read the newer copy on the next tick
            continue;
        }
        match win::is_private_copy() {
            None => {
                last_seq = -1; // clipboard busy: read this copy again on the next tick
                continue;
            }
            Some(true) => {
                last_seq = seq;
                applog::info("clipboard: skipped a private copy");
                continue;
            }
            Some(false) => {}
        }
        last_seq = seq;
        on_copy(&app, copied);
    }
}

/// Other systems: read the clipboard text and compare its hash with the last one seen.
#[cfg(not(any(windows, target_os = "macos")))]
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
            on_copy(&app, Copied::Text(text));
        }
    }
}

#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::{thread, time::Duration};

    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
        IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};
    use windows_sys::Win32::UI::Shell::DragQueryFileW;

    pub const CF_UNICODETEXT: u32 = 13;
    pub const CF_HDROP: u32 = 15;
    const CF_DIB: u32 = 8;
    /// CF_BITMAP, CF_DIB, CF_DIBV5.
    const IMAGE_FORMATS: [u32; 3] = [2, 8, 17];

    pub fn seq() -> i64 {
        // SAFETY: GetClipboardSequenceNumber only reads a counter; it needs no open clipboard.
        unsafe { GetClipboardSequenceNumber() as i64 }
    }

    pub fn format(name: &str) -> u32 {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
        unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
    }

    pub fn has(f: u32) -> bool {
        // SAFETY: IsClipboardFormatAvailable only reads the clipboard's format list.
        f != 0 && unsafe { IsClipboardFormatAvailable(f) } != 0
    }

    pub fn has_image() -> bool {
        IMAGE_FORMATS.iter().any(|&f| has(f)) || has(format("PNG"))
    }

    /// Opens the clipboard, trying for about 200 ms if another app has it open.
    fn open(owner: *mut c_void) -> bool {
        for _ in 0..10 {
            // SAFETY: OpenClipboard takes a window handle or null; the caller closes it again.
            if unsafe { OpenClipboard(owner as _) } != 0 {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        false
    }

    /// True when the copy is marked private, the way password managers and Windows itself do
    /// it: an "ExcludeClipboardContentFromMonitorProcessing" or "Clipboard Viewer Ignore"
    /// format, or "CanIncludeInClipboardHistory" set to 0. None when the clipboard could not
    /// be opened.
    pub fn is_private_copy() -> Option<bool> {
        if has(format("ExcludeClipboardContentFromMonitorProcessing")) || has(format("Clipboard Viewer Ignore")) {
            return Some(true);
        }
        let history = format("CanIncludeInClipboardHistory");
        if !has(history) {
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

    /// The first `n` bytes of a clipboard format. None when it is missing, shorter or busy.
    fn head(f: u32, n: usize) -> Option<Vec<u8>> {
        if !has(f) || !open(std::ptr::null_mut()) {
            return None;
        }
        // SAFETY: the clipboard is open until CloseClipboard below; the handle is only read
        // while locked, and only within the size GlobalSize reports.
        unsafe {
            let mut out = None;
            let data = GetClipboardData(f);
            if data as usize != 0 && GlobalSize(data as _) >= n {
                let ptr = GlobalLock(data as _) as *const u8;
                if !ptr.is_null() {
                    out = Some(std::slice::from_raw_parts(ptr, n).to_vec());
                    GlobalUnlock(data as _);
                }
            }
            CloseClipboard();
            out
        }
    }

    /// Width and height of the copied image from its PNG or DIB header (not decoded).
    pub fn image_dims() -> Option<(u32, u32)> {
        head(format("PNG"), 24)
            .and_then(|b| super::png_dims(&b))
            .or_else(|| head(CF_DIB, 12).and_then(|b| super::dib_dims(&b)))
    }

    /// The paths in CF_HDROP (Explorer's copied files). None when the clipboard is busy.
    pub fn read_files() -> Option<Vec<String>> {
        if !open(std::ptr::null_mut()) {
            return None;
        }
        let mut files = Vec::new();
        // SAFETY: the clipboard is open; the CF_HDROP handle is only used until CloseClipboard,
        // and every buffer passed to DragQueryFileW is as long as the length given with it.
        unsafe {
            let data = GetClipboardData(CF_HDROP);
            if data as usize != 0 {
                let count = DragQueryFileW(data as _, u32::MAX, std::ptr::null_mut(), 0);
                for i in 0..count {
                    let len = DragQueryFileW(data as _, i, std::ptr::null_mut(), 0);
                    let mut buf = vec![0u16; len as usize + 1];
                    let got = DragQueryFileW(data as _, i, buf.as_mut_ptr(), buf.len() as u32) as usize;
                    if got > 0 {
                        files.push(String::from_utf16_lossy(&buf[..got.min(len as usize)]));
                    }
                }
            }
            CloseClipboard();
        }
        Some(files)
    }

    /// Replaces the clipboard with the given formats. `owner` is HL Desk's window, so the
    /// device-screenshot watcher (screenshot/watch.rs) sees our process as the owner and does
    /// not offer the image as a new screenshot. Returns the sequence number after the write.
    pub fn write(owner: *mut c_void, formats: &[(u32, Vec<u8>)]) -> Result<i64, String> {
        if !open(owner) {
            return Err("The clipboard is busy. Try again.".into());
        }
        let mut result = Ok(());
        // SAFETY: the clipboard is open until CloseClipboard below. Each block is allocated,
        // filled within its size and unlocked before SetClipboardData takes it over; a block the
        // clipboard did not take is freed here.
        unsafe {
            if EmptyClipboard() == 0 {
                result = Err("Could not empty the clipboard".to_string());
            }
            for (format, bytes) in formats {
                if result.is_err() {
                    break;
                }
                let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1));
                if mem.is_null() {
                    result = Err("Out of memory".to_string());
                    break;
                }
                let ptr = GlobalLock(mem) as *mut u8;
                if ptr.is_null() {
                    GlobalFree(mem);
                    result = Err("Out of memory".to_string());
                    break;
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                GlobalUnlock(mem);
                if SetClipboardData(*format, mem as _) as usize == 0 {
                    GlobalFree(mem);
                    result = Err(format!("Windows did not accept clipboard format {format}"));
                }
            }
            CloseClipboard();
        }
        result.map(|_| seq())
    }

    /// An image as CF_DIB (32-bit BGRA, bottom-up) plus the "PNG" format that keeps
    /// transparency for apps that read it (Office, browsers, chat apps).
    pub fn image_formats(png: &[u8], rgba: &[u8], width: u32, height: u32) -> Vec<(u32, Vec<u8>)> {
        let (w, h) = (width as usize, height as usize);
        let mut dib = Vec::with_capacity(40 + w * h * 4);
        dib.extend_from_slice(&40u32.to_le_bytes()); // biSize
        dib.extend_from_slice(&(width as i32).to_le_bytes());
        dib.extend_from_slice(&(height as i32).to_le_bytes()); // positive: bottom-up rows
        dib.extend_from_slice(&1u16.to_le_bytes()); // planes
        dib.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
        dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
        dib.extend_from_slice(&[0u8; 16]); // resolution, colours used, colours important
        for row in (0..h).rev() {
            for px in rgba[row * w * 4..(row + 1) * w * 4].chunks_exact(4) {
                dib.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        }
        let mut formats = vec![(CF_DIB, dib)];
        let png_format = format("PNG");
        if png_format != 0 {
            formats.push((png_format, png.to_vec()));
        }
        formats
    }

    /// Files as CF_HDROP (a DROPFILES header and NUL-separated wide paths), marked as a copy
    /// ("Preferred DropEffect" = DROPEFFECT_COPY), so they paste into a folder or a chat.
    pub fn file_formats(paths: &[String]) -> Vec<(u32, Vec<u8>)> {
        let mut drop = Vec::new();
        drop.extend_from_slice(&20u32.to_le_bytes()); // pFiles: the list starts after the header
        drop.extend_from_slice(&[0u8; 12]); // pt (x, y), fNC
        drop.extend_from_slice(&1u32.to_le_bytes()); // fWide: UTF-16 paths
        for path in paths {
            for unit in path.encode_utf16().chain(Some(0)) {
                drop.extend_from_slice(&unit.to_le_bytes());
            }
        }
        drop.extend_from_slice(&0u16.to_le_bytes());
        let mut formats = vec![(CF_HDROP, drop)];
        let effect = format("Preferred DropEffect");
        if effect != 0 {
            formats.push((effect, 1u32.to_le_bytes().to_vec()));
        }
        formats
    }
}

/// HL Desk's main window handle, the clipboard owner for our own writes (null if not found).
#[cfg(windows)]
fn owner_window(app: &AppHandle) -> *mut std::ffi::c_void {
    app.get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map_or(std::ptr::null_mut(), |h| h.0 as _)
}

/// The clipboard change counter now, where the system has one.
fn change_count() -> Option<i64> {
    #[cfg(windows)]
    return Some(win::seq());
    #[cfg(target_os = "macos")]
    return Some(mac::change_count());
    #[cfg(not(any(windows, target_os = "macos")))]
    None
}

/// Called once at start-up, after the plugins are ready.
pub fn init(app: &AppHandle) {
    let conn = open_db(app);
    let images = app
        .path()
        .app_data_dir()
        .map(|d| d.join(IMAGES_DIR))
        .unwrap_or_else(|_| std::env::temp_dir().join("hl-desk-clipboard-images"));
    delete_expired(&conn, &images);
    remove_orphan_images(&conn, &images);
    app.manage(Clipboard {
        db: Mutex::new(conn),
        skip_hash: Mutex::new(None),
        own_change: Mutex::new(None),
        images,
    });

    let handle = app.clone();
    thread::spawn(move || watch(handle));
}

fn err(e: rusqlite::Error) -> String {
    e.to_string()
}

#[tauri::command]
fn clipboard_list(
    state: State<'_, Clipboard>,
    query: Option<String>,
    kind: Option<String>,
) -> Result<Vec<Item>, String> {
    let conn = state.db.lock().unwrap();
    let query = query.unwrap_or_default();
    let kind = kind.unwrap_or_default();
    let mut stmt = conn
        .prepare(&format!(
            "SELECT {COLUMNS} FROM items
             WHERE (?1 = '' OR instr(lower(text), lower(?1)) > 0) AND (?2 = '' OR kind = ?2)
             ORDER BY pinned DESC, created_at DESC, id DESC
             LIMIT ?3"
        ))
        .map_err(err)?;
    let rows = stmt.query_map(params![query.trim(), kind, LIST_LIMIT], item_from_row).map_err(err)?;
    let mut items = rows.collect::<Result<Vec<_>, _>>().map_err(err)?;
    for item in items.iter_mut().filter(|i| i.kind == "files") {
        item.missing = item.files.iter().filter(|p| !file_present(p)).count();
    }
    Ok(items)
}

/// An image item's thumbnail (PNG bytes).
#[tauri::command]
fn clipboard_thumb(state: State<'_, Clipboard>, id: i64) -> Result<Response, String> {
    let path: Option<String> = state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT thumb_path FROM items WHERE id = ?1", params![id], |r| r.get(0))
        .map_err(err)?;
    let path = path.ok_or("This item has no thumbnail")?;
    fs::read(path).map(Response::new).map_err(|e| e.to_string())
}

/// Puts an item back on the clipboard (without recording it again): text as text, an image as
/// an image, files as a file list. The front end shows "Copied" and then hides the window, so
/// the user can paste into the app they came from.
#[tauri::command]
fn clipboard_copy(app: AppHandle, state: State<'_, Clipboard>, id: i64) -> Result<(), String> {
    let item = state
        .db
        .lock()
        .unwrap()
        .query_row(&format!("SELECT {COLUMNS} FROM items WHERE id = ?1"), params![id], item_from_row)
        .map_err(err)?;
    // Held during the write, so the watcher cannot look at this change before it is marked.
    let mut own = state.own_change.lock().unwrap();
    let result = match item.kind.as_str() {
        "image" => copy_image(&app, &item),
        "files" => copy_files(&app, &item),
        _ => copy_text(&app, &state, &item.text),
    };
    match &result {
        Ok(count) => {
            *own = *count;
            applog::info(&format!("clipboard: copied {} item back", item.kind));
        }
        Err(e) => applog::warn(&format!("clipboard: could not copy {} item back: {e}", item.kind)),
    }
    result.map(|_| ())
}

fn copy_text(app: &AppHandle, state: &Clipboard, text: &str) -> Result<Option<i64>, String> {
    *state.skip_hash.lock().unwrap() = Some(hash(text));
    if let Err(e) = app.clipboard().write_text(text.to_string()) {
        *state.skip_hash.lock().unwrap() = None;
        return Err(e.to_string());
    }
    Ok(change_count())
}

fn copy_image(app: &AppHandle, item: &Item) -> Result<Option<i64>, String> {
    let path = item.image_path.as_deref().ok_or("The image file is missing")?;
    let png = fs::read(path).map_err(|e| format!("The image file is missing ({e})"))?;
    let rgba = xcap::image::load_from_memory_with_format(&png, ImageFormat::Png)
        .map_err(|e| format!("The image file cannot be read ({e})"))?
        .to_rgba8();
    let (width, height) = rgba.dimensions();
    #[cfg(windows)]
    {
        win::write(owner_window(app), &win::image_formats(&png, rgba.as_raw(), width, height)).map(Some)
    }
    #[cfg(not(windows))]
    {
        app.clipboard()
            .write_image(&Image::new(rgba.as_raw(), width, height))
            .map_err(|e| e.to_string())?;
        Ok(change_count())
    }
}

/// Puts back the files that still exist; fails if none does.
fn copy_files(app: &AppHandle, item: &Item) -> Result<Option<i64>, String> {
    let existing: Vec<String> = item.files.iter().filter(|p| file_present(p)).cloned().collect();
    if existing.is_empty() {
        return Err(if item.files.len() == 1 {
            "The file is no longer there".into()
        } else {
            "The files are no longer there".into()
        });
    }
    if existing.len() < item.files.len() {
        applog::info(&format!(
            "clipboard: {} of {} file(s) missing, copying the rest",
            item.files.len() - existing.len(),
            item.files.len()
        ));
    }
    #[cfg(windows)]
    return win::write(owner_window(app), &win::file_formats(&existing)).map(Some);
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        return mac::write_files(&existing).map(Some);
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = app;
        Err("Copying files back is not supported on this system".into())
    }
}

#[tauri::command]
fn clipboard_pin(state: State<'_, Clipboard>, id: i64, pinned: bool) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    conn.execute("UPDATE items SET pinned = ?1 WHERE id = ?2", params![pinned, id]).map_err(err)?;
    Ok(())
}

/// Called when a delete's undo time is over: removes the row and its image files.
#[tauri::command]
fn clipboard_delete(state: State<'_, Clipboard>, id: i64) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    delete_where(&conn, &state.images, "id = ?1", params![id]).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn clipboard_clear_unpinned(state: State<'_, Clipboard>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let deleted = delete_where(&conn, &state.images, "pinned = 0", params![]).map_err(err)?;
    applog::info(&format!("clipboard: cleared {deleted} unpinned item(s)"));
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
/// so the front end can offer "N older items removed · Undo" (clipboard_undo_keep). Their image
/// files stay until the undo time is over (clipboard_drop_files).
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
            .prepare(&format!(
                "SELECT {COLUMNS} FROM items WHERE pinned = 0 AND created_at < ?1 ORDER BY created_at, id"
            ))
            .map_err(err)?;
        let rows = stmt.query_map(params![cutoff], item_from_row).map_err(err)?;
        let removed = rows.collect::<Result<Vec<_>, _>>().map_err(err)?;
        removed
    };
    tx.execute("DELETE FROM items WHERE pinned = 0 AND created_at < ?1", params![cutoff])
        .map_err(err)?;
    tx.commit().map_err(err)?;
    applog::info(&format!(
        "clipboard: keep changed from {before} to {hours} hours, {} older item(s) removed",
        removed.len()
    ));
    Ok(removed)
}

/// The Keep change's undo time is over: deletes the image files of the items it removed
/// (only files in clipboard-images that no row uses).
#[tauri::command]
fn clipboard_drop_files(state: State<'_, Clipboard>, items: Vec<Item>) -> Result<(), String> {
    let conn = state.db.lock().unwrap();
    let paths: Vec<Option<String>> = items.into_iter().flat_map(|i| [i.image_path, i.thumb_path]).collect();
    let removed = remove_image_files(&conn, &state.images, &paths);
    if removed > 0 {
        applog::info(&format!("clipboard: deleted {removed} image file(s) after the keep change"));
    }
    Ok(())
}

/// Undo for clipboard_set_keep_hours: puts the old keep time back first (so the minute cleanup
/// cannot remove the items again), then the removed items with their own id, time and pin
/// (a new id only if a newer copy has taken that id since).
#[tauri::command]
fn clipboard_undo_keep(state: State<'_, Clipboard>, hours: i64, items: Vec<Item>) -> Result<(), String> {
    let result = undo_keep(&state, hours, &items);
    match &result {
        Ok(restored) => applog::info(&format!(
            "clipboard: keep undo: back to {hours} hours, {restored} of {} item(s) restored",
            items.len()
        )),
        Err(e) => applog::error(&format!("clipboard: keep undo failed: {e}")),
    }
    result.map(|_| ())
}

/// Whether an item sent back by the page may be stored again: sane text, image files inside
/// clipboard-images that still exist, or a non-empty file list.
fn restorable(item: &Item, images: &Path) -> bool {
    let own_file = |p: &Option<String>| {
        p.as_deref().is_some_and(|p| Path::new(p).parent() == Some(images) && Path::new(p).is_file())
    };
    match item.kind.as_str() {
        "text" => !item.text.trim().is_empty() && item.text.len() <= MAX_ITEM_BYTES,
        "image" => own_file(&item.image_path) && own_file(&item.thumb_path),
        "files" => !item.files.is_empty(),
        _ => false,
    }
}

/// Returns how many items were put back.
fn undo_keep(state: &Clipboard, hours: i64, items: &[Item]) -> Result<usize, String> {
    check_hours(hours)?;
    let conn = state.db.lock().unwrap();
    let tx = conn.unchecked_transaction().map_err(err)?;
    store_keep_hours(&tx, hours).map_err(err)?;
    let mut restored = 0;
    for item in items.iter().filter(|i| restorable(i, &state.images)) {
        let files_json =
            if item.kind == "files" { serde_json::to_string(&item.files).ok() } else { None };
        let values: [&dyn ToSql; 9] = [
            &item.kind,
            &item.text,
            &item.image_path,
            &item.thumb_path,
            &item.width,
            &item.height,
            &files_json,
            &item.created_at,
            &item.pinned,
        ];
        let mut with_id = values.to_vec();
        with_id.push(&item.id);
        let same_id = tx
            .execute(
                "INSERT OR IGNORE INTO items
                 (kind, text, image_path, thumb_path, width, height, files_json, created_at, pinned, id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                &with_id[..],
            )
            .map_err(err)?;
        if same_id == 0 {
            tx.execute(
                "INSERT INTO items
                 (kind, text, image_path, thumb_path, width, height, files_json, created_at, pinned)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                &values[..],
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
        clipboard_thumb,
        clipboard_copy,
        clipboard_pin,
        clipboard_delete,
        clipboard_clear_unpinned,
        clipboard_get_settings,
        clipboard_set_keep_hours,
        clipboard_drop_files,
        clipboard_undo_keep
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new empty folder under the system temp folder.
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hl-desk-test-{}", new_image_stem()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn db_in_memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        create_tables(&conn);
        conn
    }

    /// An image row with an image and a thumbnail file of `bytes` bytes each.
    fn add_image(conn: &Connection, images: &Path, name: &str, created_at: i64, pinned: bool, bytes: usize) -> i64 {
        let image = images.join(format!("{name}.png"));
        let thumb = images.join(format!("{name}-thumb.png"));
        fs::write(&image, vec![0u8; bytes]).unwrap();
        fs::write(&thumb, vec![0u8; bytes]).unwrap();
        conn.execute(
            "INSERT INTO items (kind, text, image_path, thumb_path, created_at, pinned)
             VALUES ('image', '', ?1, ?2, ?3, ?4)",
            params![image.to_string_lossy(), thumb.to_string_lossy(), created_at, pinned],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    fn ids(conn: &Connection) -> Vec<i64> {
        let mut stmt = conn.prepare("SELECT id FROM items ORDER BY id").unwrap();
        let rows = stmt.query_map([], |r| r.get(0)).unwrap();
        let ids: Vec<i64> = rows.map(Result::unwrap).collect();
        ids
    }

    #[test]
    fn image_names_never_repeat() {
        let names: std::collections::HashSet<String> = (0..1000).map(|_| new_image_stem()).collect();
        assert_eq!(names.len(), 1000);
        assert!(names.iter().all(|n| n.chars().all(|c| c.is_ascii_hexdigit() || c == '-')));
    }

    #[test]
    fn network_paths_are_detected() {
        assert!(is_network_path(r"\\server\share\a.txt"));
        assert!(is_network_path("//server/share/a.txt"));
        assert!(is_network_path(r"\\?\UNC\server\share\a.txt"));
        assert!(is_network_path(r"\\?\unc\server\share\a.txt"));
        assert!(!is_network_path(r"\\?\C:\a.txt"));
        assert!(!is_network_path(r"C:\Users\a.txt"));
        assert!(!is_network_path("/Users/a.txt"));
        assert!(!is_network_path(""));
        assert_eq!(drive_root(r"Z:\share\a.txt").as_deref(), Some(r"Z:\"));
        assert_eq!(drive_root("z:/a.txt").as_deref(), Some(r"z:\"));
        assert_eq!(drive_root(r"\Users\a.txt"), None);
        assert_eq!(drive_root("1:"), None);
        // The system drive is never a network drive.
        assert!(!is_network_path(r"\\?\C:\Windows\a.txt"));
        // Not looked up, so counted as present even though the server does not exist.
        assert!(file_present(r"\\hl-desk-no-such-server\share\a.txt"));
    }

    #[test]
    fn pixel_cap() {
        assert!(!too_many_pixels(7680, 4320));
        assert!(too_many_pixels(10_000, 5_001));
        assert!(too_many_pixels(u32::MAX, u32::MAX));
    }

    #[test]
    fn image_sizes_from_headers() {
        let png = png_bytes(&[0u8; 3 * 2 * 4], 3, 2).unwrap();
        assert_eq!(png_dims(&png), Some((3, 2)));
        assert_eq!(png_dims(b"not a png at all, not a png"), None);

        // Little-endian TIFF: width LONG 70000, height SHORT 300.
        let mut le = b"II*\0".to_vec();
        le.extend_from_slice(&8u32.to_le_bytes());
        le.extend_from_slice(&2u16.to_le_bytes());
        for (tag, kind, value) in [(256u16, 4u16, 70_000u32), (257, 3, 300)] {
            le.extend_from_slice(&tag.to_le_bytes());
            le.extend_from_slice(&kind.to_le_bytes());
            le.extend_from_slice(&1u32.to_le_bytes());
            le.extend_from_slice(&value.to_le_bytes()); // a SHORT sits in the first two bytes
        }
        assert_eq!(tiff_dims(&le), Some((70_000, 300)));

        // Big-endian TIFF: width SHORT 640, height LONG 480.
        let mut be = b"MM\0*".to_vec();
        be.extend_from_slice(&8u32.to_be_bytes());
        be.extend_from_slice(&2u16.to_be_bytes());
        be.extend_from_slice(&256u16.to_be_bytes());
        be.extend_from_slice(&3u16.to_be_bytes());
        be.extend_from_slice(&1u32.to_be_bytes());
        be.extend_from_slice(&[0x02, 0x80, 0, 0]);
        be.extend_from_slice(&257u16.to_be_bytes());
        be.extend_from_slice(&4u16.to_be_bytes());
        be.extend_from_slice(&1u32.to_be_bytes());
        be.extend_from_slice(&480u32.to_be_bytes());
        assert_eq!(tiff_dims(&be), Some((640, 480)));
        assert_eq!(tiff_dims(&be[..20]), None); // cut short

        let mut dib = 40u32.to_le_bytes().to_vec();
        dib.extend_from_slice(&1920i32.to_le_bytes());
        dib.extend_from_slice(&(-1080i32).to_le_bytes());
        assert_eq!(dib_dims(&dib), Some((1920, 1080)));
        assert_eq!(dib_dims(&[1, 2, 3]), None);
    }

    #[test]
    fn orphan_cleanup_leaves_files_alone_with_in_memory_db() {
        let images = temp_dir();
        let file = images.join("old.png");
        fs::write(&file, b"x").unwrap();
        let conn = db_in_memory();
        assert!(!is_file_db(&conn));
        remove_orphan_images(&conn, &images);
        assert!(file.exists());
        let _ = fs::remove_dir_all(&images);
    }

    #[test]
    fn orphan_cleanup_removes_unused_files_with_file_db() {
        let dir = temp_dir();
        let images = dir.join(IMAGES_DIR);
        fs::create_dir_all(&images).unwrap();
        let conn = Connection::open(dir.join("clipboard.db")).unwrap();
        create_tables(&conn);
        assert!(is_file_db(&conn));
        add_image(&conn, &images, "kept", 1, false, 1);
        let orphan = images.join("orphan.png");
        fs::write(&orphan, b"x").unwrap();
        remove_orphan_images(&conn, &images);
        assert!(!orphan.exists());
        assert!(images.join("kept.png").exists() && images.join("kept-thumb.png").exists());
        drop(conn);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_cap_deletes_oldest_unpinned_first() {
        let images = temp_dir();
        let conn = db_in_memory();
        // 200 bytes per image (picture + thumbnail), 600 in all.
        let pinned = add_image(&conn, &images, "a", 1, true, 100);
        add_image(&conn, &images, "b", 2, false, 100);
        let newer = add_image(&conn, &images, "c", 3, false, 100);

        assert_eq!(enforce_images_cap(&conn, &images, 600, newer), 0);
        assert_eq!(enforce_images_cap(&conn, &images, 400, newer), 1);
        assert_eq!(ids(&conn), vec![pinned, newer]);
        assert!(!images.join("b.png").exists() && !images.join("b-thumb.png").exists());
        assert!(images.join("a.png").exists() && images.join("c.png").exists());

        // Pinned images and the image just recorded are never deleted, even when the pinned
        // ones alone are over the cap.
        assert_eq!(enforce_images_cap(&conn, &images, 0, newer), 0);
        assert_eq!(ids(&conn), vec![pinned, newer]);
        assert!(images.join("a.png").exists() && images.join("c.png").exists());

        // Any other unpinned image does go.
        assert_eq!(enforce_images_cap(&conn, &images, 0, -1), 1);
        assert_eq!(ids(&conn), vec![pinned]);
        let _ = fs::remove_dir_all(&images);
    }

    #[test]
    fn size_cap_counts_only_files_rows_own() {
        let images = temp_dir();
        let conn = db_in_memory();
        let older = add_image(&conn, &images, "a", 1, false, 100);
        let newer = add_image(&conn, &images, "b", 2, false, 100);
        // A file no row owns (an orphan or one waiting for a Keep undo) is not counted, so it
        // cannot make the cap wipe the history.
        fs::write(images.join("orphan.png"), vec![0u8; 5000]).unwrap();
        assert_eq!(enforce_images_cap(&conn, &images, 400, newer), 0);
        assert_eq!(ids(&conn), vec![older, newer]);
        assert_eq!(enforce_images_cap(&conn, &images, 300, newer), 1);
        assert_eq!(ids(&conn), vec![newer]);
        assert!(images.join("orphan.png").exists());
        let _ = fs::remove_dir_all(&images);
    }
}
