//! Notes feature (Rust side). Only this file and src/features/notes/ belong to it.

use tauri::{ipc::Invoke, AppHandle};

/// Called once at start-up, after the plugins are ready.
pub fn init(_app: &AppHandle) {}

/// This feature's commands. Name each one "notes_<name>" (for example "notes_list")
/// so lib.rs routes it here, and list it in generate_handler! below.
pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![]
}
