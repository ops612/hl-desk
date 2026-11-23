//! The three global shortcuts each person can change in Settings > Shortcuts: open or close
//! HL Desk, capture an area, and the Translate window. Front end: src/shortcuts/.
//!
//! A combo is stored as modifiers then one key, joined by "+": "Ctrl+Shift+Digit2",
//! "Shift+Super+Space" (Super is Cmd on macOS, the Win key on Windows; the key is a
//! KeyboardEvent.code). settings.json "shortcuts" keeps only the changed ones; a missing or
//! unreadable entry means the default. A new combo that another app or another HL Desk shortcut
//! holds is refused and the old one is kept (registered again). At start-up a saved combo that
//! cannot be registered falls back to the default, with a warning in the log.
//!
//! The device capture key (Print Screen / Cmd+Shift+4, Settings > Capture key) is not one of
//! these; screenshot/keys.rs registers it next to the capture shortcut.
//! Commands are named "shortcuts_<name>" and routed here by lib.rs.

use std::{collections::BTreeMap, sync::Mutex};

use serde::Serialize;
use tauri::{ipc::Invoke, AppHandle, Emitter};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::applog;

pub const IDS: [&str; 3] = ["open", "capture", "translate"];
/// Sent to every window after a change: the combos by id.
const CHANGED_EVENT: &str = "shortcuts-changed";

/// The built-in combos (the keys HL Desk had before 0.1.14), in IDS order.
#[cfg(target_os = "macos")]
const DEFAULTS: [&str; 3] = ["Shift+Super+Space", "Shift+Super+Digit2", "Shift+Super+Digit1"];
#[cfg(not(target_os = "macos"))]
const DEFAULTS: [&str; 3] = ["Ctrl+Shift+Space", "Ctrl+Shift+Digit2", "Ctrl+Shift+Digit1"];

pub fn default_combo(id: &str) -> Option<&'static str> {
    IDS.iter().position(|i| *i == id).map(|n| DEFAULTS[n])
}

/// The id as a static str (for the shortcut handler); None for an unknown id.
fn static_id(id: &str) -> Option<&'static str> {
    IDS.iter().find(|i| **i == id).copied()
}

/// Keys a shortcut may end with (the same list as src/shortcuts/combo.ts): letters, digits,
/// F1-F12, Space and the punctuation keys.
fn allowed_key(code: &str) -> bool {
    const PUNCT: [&str; 11] = [
        "Minus", "Equal", "BracketLeft", "BracketRight", "Backslash", "Semicolon", "Quote", "Comma",
        "Period", "Slash", "Backquote",
    ];
    let one = |prefix: &str, ok: fn(char) -> bool| {
        code.strip_prefix(prefix)
            .is_some_and(|rest| rest.chars().count() == 1 && rest.chars().all(ok))
    };
    one("Key", |c| c.is_ascii_uppercase())
        || one("Digit", |c| c.is_ascii_digit())
        || code
            .strip_prefix('F')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=12).contains(&n) && !code.starts_with("F0"))
        || code == "Space"
        || PUNCT.contains(&code)
}

/// "Ctrl+Shift+Digit2" to a Shortcut. Needs at least one modifier and an allowed key.
pub fn parse_combo(combo: &str) -> Result<Shortcut, String> {
    let mut parts: Vec<&str> = combo.split('+').collect();
    let key = parts.pop().unwrap_or_default();
    let mut mods = Modifiers::empty();
    for part in parts {
        mods |= match part {
            "Ctrl" => Modifiers::CONTROL,
            "Alt" => Modifiers::ALT,
            "Shift" => Modifiers::SHIFT,
            "Super" => Modifiers::SUPER,
            _ => return Err(format!("unknown modifier {part:?} in {combo:?}")),
        };
    }
    if mods.is_empty() {
        return Err(format!("{combo:?} has no Ctrl, Alt, Shift or Cmd"));
    }
    if !allowed_key(key) {
        return Err(format!("{key:?} cannot be a shortcut key"));
    }
    let code = key
        .parse::<Shortcut>()
        .map_err(|e| format!("could not read the key {key:?}: {e}"))?
        .key;
    Ok(Shortcut::new(Some(mods), code))
}

fn same_keys(a: &Shortcut, b: &Shortcut) -> bool {
    a.mods == b.mods && a.key == b.key
}

/// Another of HL Desk's shortcuts (not `id`) that already uses `shortcut`.
pub fn clash<'a>(id: &str, shortcut: &Shortcut, current: &'a BTreeMap<&'static str, Entry>) -> Option<&'a str> {
    current
        .iter()
        .find(|(other, entry)| **other != id && same_keys(&entry.shortcut, shortcut))
        .map(|(other, _)| *other)
}

/// The combo saved for `id` if it reads, else the default.
pub fn effective(id: &str, saved: &BTreeMap<String, String>) -> Option<(String, Shortcut)> {
    if let Some(combo) = saved.get(id) {
        if let Ok(shortcut) = parse_combo(combo) {
            return Some((combo.clone(), shortcut));
        }
    }
    let combo = default_combo(id)?;
    parse_combo(combo).ok().map(|s| (combo.to_string(), s))
}

pub struct Entry {
    combo: String,
    shortcut: Shortcut,
}

struct State {
    current: BTreeMap<&'static str, Entry>,
    /// While a chip records a new combo, HL Desk's shortcuts are released so the keys reach the
    /// page (Windows would otherwise swallow a registered combo and run its action).
    paused: bool,
}

static STATE: Mutex<State> = Mutex::new(State { current: BTreeMap::new(), paused: false });

fn lock() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// What a shortcut does.
fn run(app: &AppHandle, id: &str) {
    match id {
        "open" => crate::toggle_window(app),
        "capture" => crate::screenshot::start_capture(app.clone()),
        "translate" => crate::translator::open_soon(app),
        _ => {}
    }
}

fn register(app: &AppHandle, id: &'static str, shortcut: Shortcut) -> Result<(), String> {
    app.global_shortcut()
        .on_shortcut(shortcut, move |app, _shortcut, event| {
            if event.state() == ShortcutState::Pressed {
                applog::info(&format!("shortcut: {id} pressed"));
                run(app, id);
            }
        })
        .map_err(|e| e.to_string())
}

fn unregister(app: &AppHandle, id: &str, shortcut: Shortcut) {
    if let Err(e) = app.global_shortcut().unregister(shortcut) {
        applog::warn(&format!("shortcuts: could not release {id}: {e}"));
    }
}

/// Called once at start-up, before the device capture key (screenshot/keys.rs).
pub fn init(app: &AppHandle) {
    let saved = crate::settings::shortcuts(app);
    let mut state = lock();
    for id in IDS {
        let default = default_combo(id).unwrap_or_default();
        let mut chosen = None;
        if let Some((combo, shortcut)) = effective(id, &saved).filter(|(c, _)| c != default) {
            if let Some(other) = clash(id, &shortcut, &state.current) {
                applog::warn(&format!("shortcuts: saved {id} {combo} is used by {other}; using the default"));
            } else {
                match register(app, id, shortcut) {
                    Ok(()) => chosen = Some(Entry { combo, shortcut }),
                    Err(e) => applog::warn(&format!(
                        "shortcuts: could not register the saved {id} {combo} (another app may use it): {e}; using the default"
                    )),
                }
            }
        } else if saved.contains_key(id) && saved.get(id).map(String::as_str) != Some(default) {
            applog::warn(&format!("shortcuts: saved {id} could not be read; using the default"));
        }
        let entry = chosen.unwrap_or_else(|| {
            let shortcut = parse_combo(default).expect("default shortcuts parse");
            // Another app may own it: keep running (the tray icon, the Capture and Translate
            // buttons still work) and say so in the log.
            match register(app, id, shortcut) {
                Ok(()) => applog::info(&format!("shortcuts: {id} {default} registered")),
                Err(e) => applog::warn(&format!(
                    "shortcuts: could not register {id} {default} (another app may use it): {e}"
                )),
            }
            Entry { combo: default.to_string(), shortcut }
        });
        state.current.insert(id, entry);
    }
}

/// Registers the shortcuts again after a pause.
fn resume(app: &AppHandle, state: &mut State) {
    if !state.paused {
        return;
    }
    state.paused = false;
    for (id, entry) in &state.current {
        if let Err(e) = register(app, *id, entry.shortcut) {
            applog::warn(&format!("shortcuts: could not register {id} {} again: {e}", entry.combo));
        }
    }
}

fn combos(state: &State) -> BTreeMap<&'static str, String> {
    state.current.iter().map(|(id, e)| (*id, e.combo.clone())).collect()
}

/// Why a change was refused. other: the HL Desk shortcut that holds the combo ("open",
/// "capture", "translate", or "device" for the capture key setting); taken: another app.
#[derive(Serialize)]
struct Refusal {
    id: &'static str,
    combo: String,
    kept: String,
    taken: bool,
    other: Option<String>,
}

#[derive(Serialize)]
struct SetResult {
    combos: BTreeMap<&'static str, String>,
    refusals: Vec<Refusal>,
}

fn changed(app: &AppHandle, state: &State) {
    let _ = app.emit(CHANGED_EVENT, combos(state));
}

#[tauri::command]
fn shortcuts_get() -> BTreeMap<&'static str, String> {
    combos(&lock())
}

/// Releases (true) or registers again (false) HL Desk's shortcuts while a chip records.
#[tauri::command]
fn shortcuts_pause(app: AppHandle, paused: bool) {
    let mut state = lock();
    if paused && !state.paused {
        for (id, entry) in &state.current {
            unregister(&app, id, entry.shortcut);
        }
        state.paused = true;
    } else if !paused {
        resume(&app, &mut state);
    }
}

/// Changes one shortcut. A combo in use is refused and the old one kept.
#[tauri::command]
fn shortcuts_set(app: AppHandle, id: String, combo: String) -> Result<SetResult, String> {
    let id = static_id(&id).ok_or_else(|| format!("unknown shortcut {id}"))?;
    let new = parse_combo(&combo)?;
    let mut state = lock();
    resume(&app, &mut state);
    let (old_combo, old) = match state.current.get(id) {
        Some(e) => (e.combo.clone(), e.shortcut),
        None => return Err(format!("{id} is not set up")),
    };
    let refuse = |state: &State, taken: bool, other: Option<String>| SetResult {
        combos: combos(state),
        refusals: vec![Refusal { id, combo: combo.clone(), kept: old_combo.clone(), taken, other }],
    };
    if same_keys(&old, &new) {
        return Ok(SetResult { combos: combos(&state), refusals: vec![] });
    }
    if let Some(other) = clash(id, &new, &state.current) {
        let other = other.to_string();
        applog::info(&format!("shortcuts: {id} {combo} refused, used by {other}"));
        return Ok(refuse(&*state, false, Some(other)));
    }
    if app.global_shortcut().is_registered(new) {
        // Registered by HL Desk but none of these three: the device capture key.
        applog::info(&format!("shortcuts: {id} {combo} refused, used by the device capture key"));
        return Ok(refuse(&*state, false, Some("device".into())));
    }
    unregister(&app, id, old);
    if let Err(e) = register(&app, id, new) {
        applog::warn(&format!("shortcuts: could not register {id} {combo} (another app may use it): {e}"));
        if let Err(e) = register(&app, id, old) {
            applog::error(&format!("shortcuts: could not register {id} {old_combo} again: {e}"));
        }
        return Ok(refuse(&*state, true, None));
    }
    let is_default = default_combo(id).and_then(|d| parse_combo(d).ok()).is_some_and(|d| same_keys(&d, &new));
    if let Err(e) = crate::settings::set_shortcut(&app, id, (!is_default).then_some(combo.as_str())) {
        applog::error(&format!("shortcuts: could not save {id}: {e}"));
    }
    applog::info(&format!("shortcuts: {id} is now {combo}"));
    state.current.insert(id, Entry { combo, shortcut: new });
    changed(&app, &state);
    Ok(SetResult { combos: combos(&state), refusals: vec![] })
}

/// "Reset all to defaults": every changed shortcut back to its default. One whose default
/// cannot be registered keeps its combo and gets a refusal line.
#[tauri::command]
fn shortcuts_reset(app: AppHandle) -> SetResult {
    let mut state = lock();
    resume(&app, &mut state);
    let to_reset: Vec<(&'static str, Entry, Shortcut)> = IDS
        .iter()
        .filter_map(|id| {
            let default = parse_combo(default_combo(id)?).ok()?;
            let entry = state.current.get(id)?;
            (!same_keys(&entry.shortcut, &default)).then(|| {
                (*id, Entry { combo: entry.combo.clone(), shortcut: entry.shortcut }, default)
            })
        })
        .collect();
    // Release all first, so two swapped shortcuts can both go back.
    for (id, entry, _) in &to_reset {
        unregister(&app, id, entry.shortcut);
    }
    let mut refusals = vec![];
    for (id, old, default) in to_reset {
        let def = default_combo(id).unwrap_or_default().to_string();
        let device = app.global_shortcut().is_registered(default);
        let result = if device { Err("used by the device capture key".to_string()) } else { register(&app, id, default) };
        match result {
            Ok(()) => {
                if let Err(e) = crate::settings::set_shortcut(&app, id, None) {
                    applog::error(&format!("shortcuts: could not save {id}: {e}"));
                }
                applog::info(&format!("shortcuts: {id} reset to {def}"));
                state.current.insert(id, Entry { combo: def, shortcut: default });
            }
            Err(e) => {
                applog::warn(&format!("shortcuts: could not reset {id} to {def}: {e}"));
                if let Err(e) = register(&app, id, old.shortcut) {
                    applog::error(&format!("shortcuts: could not register {id} {} again: {e}", old.combo));
                }
                refusals.push(Refusal {
                    id,
                    combo: def,
                    kept: old.combo,
                    taken: !device,
                    other: device.then(|| "device".to_string()),
                });
            }
        }
    }
    changed(&app, &state);
    SetResult { combos: combos(&state), refusals }
}

pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![shortcuts_get, shortcuts_pause, shortcuts_set, shortcuts_reset]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_global_shortcut::Code;

    #[cfg(target_os = "macos")]
    const MAIN: Modifiers = Modifiers::SUPER;
    #[cfg(not(target_os = "macos"))]
    const MAIN: Modifiers = Modifiers::CONTROL;

    #[test]
    fn defaults_are_the_old_keys() {
        let open = parse_combo(default_combo("open").unwrap()).unwrap();
        assert_eq!((open.mods, open.key), (MAIN | Modifiers::SHIFT, Code::Space));
        let capture = parse_combo(default_combo("capture").unwrap()).unwrap();
        assert_eq!((capture.mods, capture.key), (MAIN | Modifiers::SHIFT, Code::Digit2));
        let translate = parse_combo(default_combo("translate").unwrap()).unwrap();
        assert_eq!((translate.mods, translate.key), (MAIN | Modifiers::SHIFT, Code::Digit1));
        assert_eq!(default_combo("nope"), None);
    }

    #[test]
    fn parses_every_allowed_key() {
        let cases = [
            ("Ctrl+Alt+KeyT", Modifiers::CONTROL | Modifiers::ALT, Code::KeyT),
            ("Shift+Super+Digit1", Modifiers::SHIFT | Modifiers::SUPER, Code::Digit1),
            ("Alt+F12", Modifiers::ALT, Code::F12),
            ("Ctrl+F1", Modifiers::CONTROL, Code::F1),
            ("Ctrl+Shift+Space", Modifiers::CONTROL | Modifiers::SHIFT, Code::Space),
            ("Ctrl+Minus", Modifiers::CONTROL, Code::Minus),
            ("Ctrl+Equal", Modifiers::CONTROL, Code::Equal),
            ("Ctrl+BracketLeft", Modifiers::CONTROL, Code::BracketLeft),
            ("Ctrl+BracketRight", Modifiers::CONTROL, Code::BracketRight),
            ("Ctrl+Backslash", Modifiers::CONTROL, Code::Backslash),
            ("Ctrl+Semicolon", Modifiers::CONTROL, Code::Semicolon),
            ("Ctrl+Quote", Modifiers::CONTROL, Code::Quote),
            ("Ctrl+Comma", Modifiers::CONTROL, Code::Comma),
            ("Ctrl+Period", Modifiers::CONTROL, Code::Period),
            ("Ctrl+Slash", Modifiers::CONTROL, Code::Slash),
            ("Ctrl+Backquote", Modifiers::CONTROL, Code::Backquote),
            ("Ctrl+KeyA", Modifiers::CONTROL, Code::KeyA),
            ("Ctrl+KeyZ", Modifiers::CONTROL, Code::KeyZ),
            ("Ctrl+Digit0", Modifiers::CONTROL, Code::Digit0),
            ("Ctrl+Digit9", Modifiers::CONTROL, Code::Digit9),
        ];
        for (combo, mods, key) in cases {
            let s = parse_combo(combo).unwrap_or_else(|e| panic!("{combo}: {e}"));
            assert_eq!((s.mods, s.key), (mods, key), "{combo}");
        }
    }

    #[test]
    fn rejects_bad_combos() {
        for combo in ["KeyT", "Digit2", "", "Ctrl+", "Hyper+KeyT", "Ctrl+Enter", "Ctrl+ArrowUp", "Ctrl+F13", "Ctrl+F0", "Ctrl+KeyAB", "Ctrl+keyt"] {
            assert!(parse_combo(combo).is_err(), "{combo} should be refused");
        }
    }

    #[test]
    fn order_of_modifiers_does_not_matter() {
        let a = parse_combo("Shift+Ctrl+KeyT").unwrap();
        let b = parse_combo("Ctrl+Shift+KeyT").unwrap();
        assert!(same_keys(&a, &b));
    }

    fn table(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, Entry> {
        pairs
            .iter()
            .map(|(id, combo)| (*id, Entry { combo: combo.to_string(), shortcut: parse_combo(combo).unwrap() }))
            .collect()
    }

    #[test]
    fn finds_a_clash_with_another_hl_desk_shortcut() {
        let current = table(&[
            ("open", "Ctrl+Shift+Space"),
            ("capture", "Ctrl+Shift+Digit2"),
            ("translate", "Ctrl+Shift+Digit1"),
        ]);
        let one = parse_combo("Shift+Ctrl+Digit1").unwrap();
        assert_eq!(clash("capture", &one, &current), Some("translate"));
        // Its own combo is no clash.
        assert_eq!(clash("translate", &one, &current), None);
        let free = parse_combo("Ctrl+Alt+KeyT").unwrap();
        assert_eq!(clash("open", &free, &current), None);
    }

    #[test]
    fn missing_or_bad_saved_combo_falls_back_to_the_default() {
        let mut saved = BTreeMap::new();
        assert_eq!(effective("open", &saved).unwrap().0, default_combo("open").unwrap());
        saved.insert("open".to_string(), "Ctrl+Alt+KeyO".to_string());
        saved.insert("capture".to_string(), "KeyX".to_string());
        assert_eq!(effective("open", &saved).unwrap().0, "Ctrl+Alt+KeyO");
        assert_eq!(effective("capture", &saved).unwrap().0, default_combo("capture").unwrap());
        assert!(effective("nope", &saved).is_none());
    }
}
