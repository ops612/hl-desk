//! Text translation (Rust side). Front end: translate.html, src/translate/, public/translate/
//! (the Bergamot engine, MPL-2.0, see THIRD_PARTY_NOTICES.md). Capability: translate.json.
//!
//! Trigger: the same text copied twice within DOUBLE_COPY_MS (Ctrl+C twice, Cmd+C twice), no
//! keyboard hook. The detector is the pure `is_double_copy`; copies come in two ways:
//! - Windows: the clipboard watcher in clipboard.rs polls every 500 ms, so two quick copies usually
//!   look like one change there. This module runs its own clipboard listener instead
//!   (AddClipboardFormatListener on a message-only window): one WM_CLIPBOARDUPDATE per copy.
//! - macOS (and others): clipboard.rs polls the pasteboard changeCount every 100 ms and calls
//!   `on_text_copied` with each new text (two looks with the same text within DOUBLE_COPY_MS are a
//!   double copy; changes within one 100 ms look are one copy, as some apps write twice per
//!   Cmd+C), and `on_other_copied` for an image or files.
//!
//! The card is one reusable window (label "translate"), created hidden on first use, shown near
//! the mouse pointer on the screen under it and always on top once its page has drawn
//! (translate_card_show), hidden again on Esc, close, Copy, a click elsewhere or the next single
//! copy (unless pinned; a new double copy reuses it). macOS: shown without activating HL Desk
//! (the keyboard stays in the user's app until they click the card).
//! Windows: shown and given the keyboard. Its page keeps the engine in a Web Worker and stops it
//! after 2 minutes idle.
//!
//! Languages: Han text -> English (zh-Hans when a Simplified-only character is found, else the last
//! pick for Han text, else zh-Hant); Latin text -> Chinese (Traditional).
//!
//! Models: downloaded on first use of a pair into <app data>/models/<pair>/ (temp file `.part`,
//! resumed with a Range request, sha256 checked against MODELS, then renamed). Download URLs come
//! from MODEL_BASE_URL (our mirror of Mozilla's files) via `model_url`, so the host can be
//! swapped in one place.
//! Every step is logged (sizes and counts only, never the copied text).

use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{
    ipc::{Channel, Invoke, Response},
    AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::{applog, settings};

// ---------- double-copy detector ----------

/// Two copies of the same text at most this far apart open the card. On macOS the watcher looks
/// every 100 ms, so a copy is seen up to about 100 ms late.
pub const DOUBLE_COPY_MS: u64 = 650;
/// Two clipboard updates closer than this are one copy (some apps write the clipboard twice).
pub const MIN_GAP_MS: u64 = 40;
/// Our own Copy (translate_copy) is ignored for this long.
const OWN_COPY_MS: u64 = 5_000;
/// Longer texts get a message instead of a translation.
pub const MAX_CHARS: usize = 5_000;

/// One text copy: a hash of the text (never the text) and when it was seen (ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub hash: u64,
    pub at_ms: u64,
}

/// True when `now` completes a double copy: the previous copy had the same text, at least
/// MIN_GAP_MS and at most DOUBLE_COPY_MS before.
pub fn is_double_copy(prev: Option<Seen>, now: Seen) -> bool {
    match prev {
        Some(p) if p.hash == now.hash && now.at_ms >= p.at_ms => {
            let gap = now.at_ms - p.at_ms;
            (MIN_GAP_MS..=DOUBLE_COPY_MS).contains(&gap)
        }
        _ => false,
    }
}

/// The last text copy that did not open the card.
static LAST: Mutex<Option<Seen>> = Mutex::new(None);
/// Hash and time of the translation our Copy button put on the clipboard.
static OWN: Mutex<Option<Seen>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

/// Entry point for the clipboard watcher (clipboard.rs, macOS): each new text copy.
/// Windows ignores it: this module's own listener sees every copy there.
#[cfg_attr(windows, allow(dead_code))] // called from the macOS watcher only
pub fn on_text_copied(app: &AppHandle, text: &str) {
    if cfg!(windows) {
        return;
    }
    handle_copy(app, text);
}

/// Entry point for the clipboard watcher (clipboard.rs, macOS): an image or files were copied.
#[cfg_attr(windows, allow(dead_code))] // called from the macOS watcher only
pub fn on_other_copied(app: &AppHandle) {
    if cfg!(windows) {
        return;
    }
    other_copy(app);
}

/// Something other than text was copied: a text copy after it is a first copy again, and an
/// unpinned card hides.
fn other_copy(app: &AppHandle) {
    *lock(&LAST) = None;
    single_copy(app);
}

/// A single copy (not a double one): tell a shown card, which hides unless pinned.
fn single_copy(app: &AppHandle) {
    let shown = app
        .get_webview_window(CARD_LABEL)
        .is_some_and(|w| w.is_visible().unwrap_or(false));
    if shown {
        let _ = app.emit_to(CARD_LABEL, COPIED_EVENT, ());
    }
}

fn handle_copy(app: &AppHandle, text: &str) {
    if text.trim().is_empty() {
        other_copy(app);
        return;
    }
    let seen = Seen { hash: hash(text), at_ms: now_ms() };
    {
        let mut own = lock(&OWN);
        if let Some(o) = *own {
            if o.hash == seen.hash && seen.at_ms.saturating_sub(o.at_ms) <= OWN_COPY_MS {
                *own = None;
                applog::info("translate: skipped our own copy");
                return;
            }
        }
    }
    let double = {
        let mut last = lock(&LAST);
        let double = is_double_copy(*last, seen);
        *last = if double { None } else { Some(seen) };
        double
    };
    if double {
        applog::info(&format!(
            "translate: double copy ({} chars)",
            text.chars().count()
        ));
        open_card(app, text);
    } else {
        single_copy(app);
    }
}

// ---------- language detection ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pair {
    ZhHantEn,
    ZhHansEn,
    EnZhHant,
}

impl Pair {
    pub const ALL: [Pair; 3] = [Pair::ZhHantEn, Pair::ZhHansEn, Pair::EnZhHant];

    pub fn id(self) -> &'static str {
        match self {
            Pair::ZhHantEn => "zh-Hant-en",
            Pair::ZhHansEn => "zh-Hans-en",
            Pair::EnZhHant => "en-zh-Hant",
        }
    }

    pub fn from_id(id: &str) -> Option<Pair> {
        Pair::ALL.into_iter().find(|p| p.id() == id)
    }

    /// The script of the text this pair translates from.
    pub fn source(self) -> Script {
        match self {
            Pair::ZhHantEn | Pair::ZhHansEn => Script::Han,
            Pair::EnZhHant => Script::Latin,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Script {
    Han,
    Latin,
}

impl Script {
    pub fn id(self) -> &'static str {
        match self {
            Script::Han => "han",
            Script::Latin => "latin",
        }
    }

    pub fn from_id(id: &str) -> Option<Script> {
        match id {
            "han" => Some(Script::Han),
            "latin" => Some(Script::Latin),
            _ => None,
        }
    }
}

/// Common characters written only in Simplified Chinese (their Traditional forms differ, and they
/// are not normal Traditional characters themselves). One of them in a text means zh-Hans.
pub const SIMPLIFIED_ONLY: &str = "们这个说时国会对来发经过还进动样现学问为门车东长见书买卖读语话请让记设证认\
谢该钱银错单员师实写间关开题图场处备将层岁带应张强报换无机条标电点热爱种积简类级纪约红线组细终结给\
络统续绿编罗义习乱产亲从价众优伤传体儿党兴养办务劳区医华协历压县参双变号吗启响园围圆坏块际陈难须顺\
顾预领风饭马鸟鱼么万与专业两严举乐网运远连选适达边订计讨议论识调谈质货费资赛转软输辆较闻阳队阶陆随\
页项颜飞馆验鸡黄没气汉满灯环确离称稳笔节";

pub fn is_han(c: char) -> bool {
    matches!(c as u32, 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F)
}

fn is_latin_letter(c: char) -> bool {
    c.is_alphabetic() && (c as u32) < 0x250
}

/// The script of a text and, when the text settles it, the pair to use. Han wins when its
/// characters are at least a third as many as the Latin letters (a Chinese word is 1 to 2
/// characters, an English word about 5 letters). None: no Han characters and no Latin letters.
/// Han text without a Simplified-only character is ambiguous (most characters are shared), so the
/// pair is left to the caller (the last pick for Han text, else zh-Hant).
pub fn detect(text: &str) -> Option<(Script, Option<Pair>)> {
    let (mut han, mut latin, mut simplified) = (0usize, 0usize, 0usize);
    for c in text.chars() {
        if is_han(c) {
            han += 1;
            if SIMPLIFIED_ONLY.contains(c) {
                simplified += 1;
            }
        } else if is_latin_letter(c) {
            latin += 1;
        }
    }
    if han == 0 && latin == 0 {
        return None;
    }
    if han * 3 >= latin {
        let pair = (simplified > 0).then_some(Pair::ZhHansEn);
        Some((Script::Han, pair))
    } else {
        Some((Script::Latin, Some(Pair::EnZhHant)))
    }
}

/// The pair for a detected text: what the text settles, else the last manual pick for that
/// script (if it translates from that script), else the default.
pub fn choose_pair(script: Script, settled: Option<Pair>, pick: Option<&str>) -> Pair {
    if let Some(p) = settled {
        if script == Script::Han || pick.is_none() {
            return p;
        }
    }
    if let Some(p) = pick.and_then(Pair::from_id) {
        if p.source() == script {
            return p;
        }
    }
    settled.unwrap_or(match script {
        Script::Han => Pair::ZhHantEn,
        Script::Latin => Pair::EnZhHant,
    })
}

// ---------- model manifest ----------

/// Where model files are downloaded from: our mirror, release "models-v1" on the public releases
/// repo, byte-identical to Mozilla's Remote Settings files. Each file is `<pair>.<name>` under
/// this base (`model_url`); swap the host here.
pub const MODEL_BASE_URL: &str = "https://github.com/ops612/hl-desk-releases/releases/download/models-v1/";

/// The download URL of one model file of `pair`.
pub fn model_url(pair: Pair, file: &ModelFile) -> String {
    format!("{MODEL_BASE_URL}{}.{}", pair.id(), file.name)
}

pub struct ModelFile {
    /// Bergamot file type: model, lex, vocab, srcvocab, trgvocab.
    pub kind: &'static str,
    pub name: &'static str,
    pub size: u64,
    pub sha256: &'static str,
}

pub struct PairModels {
    pub pair: Pair,
    /// Remote Settings record version.
    pub version: &'static str,
    pub files: &'static [ModelFile],
}

impl PairModels {
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }
}

/// Pinned models: Mozilla Remote Settings "translations-models", newest desktop release records
/// (read 2026-11-18 with the spike's fetch-models.mjs logic), mirrored at MODEL_BASE_URL.
/// Licence MPL-2.0.
pub const MODELS: [PairModels; 3] = [
    PairModels {
        pair: Pair::ZhHantEn,
        version: "2.0",
        files: &[
            ModelFile {
                kind: "model",
                name: "model.zhen.intgemm.alphas.bin",
                size: 43_849_787,
                sha256: "0aee91790894458f5d367551f6edcd4c9cb97852c34f221bcbf9f4701ebcf0cd",
            },
            ModelFile {
                kind: "lex",
                name: "lex.50.50.zhen.s2t.bin",
                size: 6_385_944,
                sha256: "aa7daf6cfc85c0cd2c10e2944d66f6da55497c9c6408789f3adfded4074c2fb1",
            },
            ModelFile {
                kind: "srcvocab",
                name: "srcvocab.zhen.spm",
                size: 769_669,
                sha256: "5cc6a76611dbf86219f109141533606b15ecb34eee83673bb86b2c16b14734db",
            },
            ModelFile {
                kind: "trgvocab",
                name: "trgvocab.zhen.spm",
                size: 812_572,
                sha256: "7bf002db37c10d3b114cc5588d7fdcb16c57d0fd1e2c34354c22cc9f0b6c3c29",
            },
        ],
    },
    PairModels {
        pair: Pair::ZhHansEn,
        version: "2.0",
        files: &[
            ModelFile {
                kind: "model",
                name: "model.zhen.intgemm.alphas.bin",
                size: 59_504_955,
                sha256: "3535442962ec8f4a553cc19b206befcac689ee9cddaea44fa91e21527fc30ac2",
            },
            ModelFile {
                kind: "lex",
                name: "lex.50.50.zhen.s2t.bin",
                size: 9_220_016,
                sha256: "cdcad3592dc2bc4676c34c4d37203f7649ee989195cf083cbb60f1ea011f976b",
            },
            ModelFile {
                kind: "vocab",
                name: "vocab.zhen.spm",
                size: 1_359_697,
                sha256: "dff594318ab7d8b7b60b844ab98ebe6b932ae8045fab15235404c787715965b3",
            },
        ],
    },
    PairModels {
        pair: Pair::EnZhHant,
        version: "2.0",
        files: &[
            ModelFile {
                kind: "model",
                name: "model.enzh.intgemm.alphas.bin",
                size: 43_849_787,
                sha256: "559ab90d723a58c1f1e2ab7cc12137bc667af5ba3e325e3eb30b5cdc930db520",
            },
            ModelFile {
                kind: "lex",
                name: "lex.50.50.enzh.s2t.bin",
                size: 4_057_188,
                sha256: "d891404d1436a7334df12539fe30a26f9e9f2b80bd42fdb8b5f8849e8a1e942b",
            },
            ModelFile {
                kind: "srcvocab",
                name: "srcvocab.enzh.spm",
                size: 803_694,
                sha256: "2266df70492162a249ab1c0154f929bd6098b246544c666c1a0d5a24dde7d2ea",
            },
            ModelFile {
                kind: "trgvocab",
                name: "trgvocab.enzh.spm",
                size: 751_671,
                sha256: "22b91a4436d70b91ab8777c677252ab5fae2bc284d71f977df5206c110e3444c",
            },
        ],
    },
];

pub fn models_for(pair: Pair) -> &'static PairModels {
    MODELS.iter().find(|m| m.pair == pair).expect("every pair has models")
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// sha256 of a file, as lowercase hex.
pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(to_hex(&hasher.finalize()))
}

/// Ok when the file has the pinned size and sha256.
pub fn verify_file(path: &Path, size: u64, sha256: &str) -> Result<(), String> {
    let len = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if len != size {
        return Err(format!("size {len}, expected {size}"));
    }
    let got = sha256_file(path).map_err(|e| e.to_string())?;
    if got != sha256 {
        return Err("checksum does not match".into());
    }
    Ok(())
}

fn pair_dir(app: &AppHandle, pair: Pair) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok(dir.join("models").join(pair.id()))
}

/// Installed: every file of the pair is there with its pinned size (the sha256 was checked when
/// it was downloaded).
fn installed(dir: &Path, models: &PairModels) -> bool {
    models
        .files
        .iter()
        .all(|f| fs::metadata(dir.join(f.name)).is_ok_and(|m| m.len() == f.size))
}

// ---------- download ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Progress {
    done: u64,
    total: u64,
}

/// A running download of one pair. A second request for the same pair (the card and an editor
/// window) joins it: its channel gets the progress too and it ends with the same result.
struct Download {
    listeners: Mutex<Vec<Channel<Progress>>>,
    last: Mutex<Progress>,
    result: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

impl Download {
    fn report(&self, progress: Progress) {
        *lock(&self.last) = progress.clone();
        for channel in lock(&self.listeners).iter() {
            let _ = channel.send(progress.clone());
        }
    }
}

static DOWNLOADS: Mutex<Vec<(Pair, std::sync::Arc<Download>)>> = Mutex::new(Vec::new());

fn http_client() -> Result<reqwest::Client, String> {
    // reqwest is built without a TLS crypto provider (as in tauri-plugin-updater): use ring.
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }
    reqwest::Client::builder()
        .user_agent(concat!("HL-Desk/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| e.to_string())
}

/// Downloads one file into `<dir>/<name>.part` (continuing a partial one with a Range request),
/// checks its size and sha256 and renames it to `<dir>/<name>`. `report(bytes of this file)`.
async fn download_file(
    client: &reqwest::Client,
    dir: &Path,
    pair: Pair,
    file: &ModelFile,
    report: &(dyn Fn(u64) + Sync),
) -> Result<(), String> {
    let part = dir.join(format!("{}.part", file.name));
    let target = dir.join(file.name);
    let mut have = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > file.size {
        let _ = fs::remove_file(&part);
        have = 0;
    }
    if have < file.size {
        // GitHub answers with a redirect to its file host; reqwest follows it and keeps Range.
        let url = model_url(pair, file);
        let mut request = client.get(&url);
        if have > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().await.map_err(|e| format!("could not connect ({e})"))?;
        let status = response.status();
        let append = if status == reqwest::StatusCode::PARTIAL_CONTENT {
            applog::info(&format!("translate: resuming {} at {have} bytes", file.name));
            true
        } else if status.is_success() {
            have = 0;
            false
        } else {
            return Err(format!("the server answered {status}"));
        };
        let mut out = if append {
            fs::OpenOptions::new().append(true).open(&part)
        } else {
            fs::File::create(&part)
        }
        .map_err(|e| format!("could not write {}: {e}", part.display()))?;
        let mut last_report = have;
        report(have);
        while let Some(chunk) = response.chunk().await.map_err(|e| format!("download stopped ({e})"))? {
            out.write_all(&chunk).map_err(|e| format!("could not write {}: {e}", part.display()))?;
            have += chunk.len() as u64;
            if have > file.size {
                drop(out);
                let _ = fs::remove_file(&part);
                return Err(format!("{} is larger than expected", file.name));
            }
            if have - last_report >= 256 * 1024 {
                last_report = have;
                report(have);
            }
        }
        out.flush().map_err(|e| e.to_string())?;
        drop(out);
        report(have);
        if have < file.size {
            return Err(format!("download stopped at {have} of {} bytes", file.size));
        }
    }
    let (check_path, size, sha) = (part.clone(), file.size, file.sha256);
    let checked = tauri::async_runtime::spawn_blocking(move || verify_file(&check_path, size, sha))
        .await
        .map_err(|e| e.to_string())?;
    if let Err(e) = checked {
        let _ = fs::remove_file(&part);
        applog::error(&format!("translate: {} failed its check ({e}); deleted", file.name));
        return Err(format!("{} failed its check; try again", file.name));
    }
    fs::rename(&part, &target).map_err(|e| format!("could not save {}: {e}", file.name))?;
    applog::info(&format!("translate: {} saved ({} bytes, sha256 ok)", file.name, file.size));
    Ok(())
}

async fn download_pair(app: &AppHandle, pair: Pair, progress: &(dyn Fn(Progress) + Sync)) -> Result<(), String> {
    let models = models_for(pair);
    let dir = pair_dir(app, pair)?;
    fs::create_dir_all(&dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let total = models.total_size();
    let client = http_client()?;
    let mut done = 0u64;
    for file in models.files {
        if fs::metadata(dir.join(file.name)).is_ok_and(|m| m.len() == file.size) {
            done += file.size;
            progress(Progress { done, total });
            continue;
        }
        let before = done;
        let report = |bytes: u64| {
            progress(Progress { done: before + bytes, total });
        };
        download_file(&client, &dir, pair, file, &report).await?;
        done += file.size;
    }
    progress(Progress { done: total, total });
    Ok(())
}

// ---------- card window ----------

const CARD_LABEL: &str = "translate";
const OPEN_EVENT: &str = "translate://open";
/// A single copy elsewhere: an unpinned card hides (a new double copy reuses it).
const COPIED_EVENT: &str = "translate://copied";
/// Card width and first height in points; the page reports its real height.
const CARD_W: f64 = 360.0;
const CARD_H: f64 = 220.0;
const CARD_MAX_H: f64 = 520.0;
/// Gap between the pointer and the card, in points.
const POINTER_GAP: f64 = 12.0;
/// A card page that has not reported by then is logged.
const SHOW_GRACE: Duration = Duration::from_secs(8);

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CardRequest {
    text: String,
    pair: &'static str,
    script: &'static str,
    too_long: bool,
}

/// The text waiting for the card page (translate_card_take).
static PENDING: Mutex<Option<CardRequest>> = Mutex::new(None);
/// The pointer position (physical pixels) when the card was asked for (not macOS).
#[cfg(not(target_os = "macos"))]
static ANCHOR: Mutex<Option<(f64, f64)>> = Mutex::new(None);
/// The card page has drawn at least once (set by translate_card_show).
static PAGE_READY: AtomicBool = AtomicBool::new(false);
/// Where the card of this card session went (top-left corner and the area it must stay in; points
/// on macOS, physical pixels elsewhere). Set by the first show after a double copy, cleared by the
/// next double copy and by hiding, so later shows of the same session only resize.
static SESSION: Mutex<Option<((i32, i32), (i32, i32, i32, i32))>> = Mutex::new(None);

fn open_card(app: &AppHandle, text: &str) {
    let Some((script, settled)) = detect(text) else {
        applog::info("translate: no Chinese characters or Latin letters in the copy; no card");
        return;
    };
    let pick = settings::translate_pick(app, script.id());
    let pair = choose_pair(script, settled, pick.as_deref());
    let chars = text.chars().count();
    applog::info(&format!(
        "translate: card for {} text, pair {} ({})",
        script.id(),
        pair.id(),
        if settled.is_some() { "detected" } else if pick.is_some() { "last pick" } else { "default" }
    ));
    *lock(&SESSION) = None;
    *lock(&PENDING) = Some(CardRequest {
        text: text.to_string(),
        pair: pair.id(),
        script: script.id(),
        too_long: chars > MAX_CHARS,
    });
    // macOS reads the pointer itself when the card is shown (translate_card_show).
    #[cfg(not(target_os = "macos"))]
    {
        *lock(&ANCHOR) = match app.cursor_position() {
            Ok(p) => Some((p.x, p.y)),
            Err(e) => {
                applog::warn(&format!("translate: no pointer position ({e}); card goes to the corner"));
                None
            }
        };
    }

    if app.get_webview_window(CARD_LABEL).is_some() {
        if let Err(e) = app.emit_to(CARD_LABEL, OPEN_EVENT, ()) {
            applog::error(&format!("translate: could not tell the card about the copy: {e}"));
        }
        return;
    }
    let builder = WebviewWindowBuilder::new(app, CARD_LABEL, WebviewUrl::App("translate.html".into()))
        .title("HL Desk Translate")
        .inner_size(CARD_W, CARD_H)
        .decorations(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false);
    // Windows: the page draws its own 1px border (no undecorated shadow), as the toast does.
    #[cfg(windows)]
    let builder = builder.shadow(false);
    // macOS: also over other Spaces and full-screen apps.
    #[cfg(not(windows))]
    let builder = builder.shadow(true).visible_on_all_workspaces(true);
    match builder.build() {
        Ok(_) => applog::info("translate: card window created"),
        Err(e) => {
            applog::error(&format!("translate: could not create the card window: {e}"));
            return;
        }
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(SHOW_GRACE);
        if !PAGE_READY.load(Ordering::SeqCst) && app.get_webview_window(CARD_LABEL).is_some() {
            applog::error(&format!(
                "translate: card page did not report ready within {} s",
                SHOW_GRACE.as_secs()
            ));
        }
    });
}

/// Top-left corner (physical pixels) for a `size` card near `pointer`, inside `area`
/// (x, y, width, height): below and right of the pointer, above it when there is no room below,
/// always inside the area.
pub fn card_position(pointer: (i32, i32), size: (i32, i32), area: (i32, i32, i32, i32), gap: i32) -> (i32, i32) {
    let (ax, ay, aw, ah) = area;
    let (w, h) = size;
    let x = pointer.0 + gap;
    let mut y = pointer.1 + gap;
    if y + h > ay + ah {
        y = pointer.1 - gap - h;
    }
    let x = x.min(ax + aw - w).max(ax);
    let y = y.min(ay + ah - h).max(ay);
    (x, y)
}

/// A card that keeps its place (same card session) but changed size: pulled back inside `area` if
/// it no longer fits.
pub fn keep_inside(pos: (i32, i32), size: (i32, i32), area: (i32, i32, i32, i32)) -> (i32, i32) {
    let (ax, ay, aw, ah) = area;
    (pos.0.min(ax + aw - size.0).max(ax), pos.1.min(ay + ah - size.1).max(ay))
}

/// The card's corner for this show: the session's place (resized, kept inside its area), or for the
/// first show of a session `first()` (corner, area), which is remembered.
fn session_position(size: (i32, i32), first: impl FnOnce() -> ((i32, i32), (i32, i32, i32, i32))) -> (i32, i32) {
    let mut session = lock(&SESSION);
    if let Some((pos, area)) = *session {
        return keep_inside(pos, size, area);
    }
    let (pos, area) = first();
    *session = Some((pos, area));
    pos
}

/// Where the card goes, in physical pixels: (x, y, width, height). On the monitor under the
/// pointer (the main one if unknown), near the pointer (top-right corner if unknown).
#[cfg(not(target_os = "macos"))]
fn placement(app: &AppHandle, height: f64) -> (i32, i32, i32, i32) {
    let anchor = *lock(&ANCHOR);
    let monitors = app.available_monitors().unwrap_or_default();
    let contains = |m: &tauri::Monitor, (x, y): (f64, f64)| {
        let (p, s) = (m.position(), m.size());
        x >= p.x as f64 && y >= p.y as f64 && x < p.x as f64 + s.width as f64 && y < p.y as f64 + s.height as f64
    };
    let monitor = anchor
        .and_then(|a| monitors.iter().find(|m| contains(*m, a)).cloned())
        .or_else(|| app.primary_monitor().ok().flatten());
    let scale = monitor.as_ref().map_or(1.0, |m| m.scale_factor());
    let px = |v: f64| (v * scale).round() as i32;
    let (w, h) = (px(CARD_W), px(height));
    let area = monitor.as_ref().map_or((0, 0, px(1280.0), px(800.0)), |m| {
        let a = m.work_area();
        (a.position.x, a.position.y, a.size.width as i32, a.size.height as i32)
    });
    let (x, y) = session_position((w, h), || {
        let pointer = anchor.map_or((area.0 + area.2, area.1), |(x, y)| (x.round() as i32, y.round() as i32));
        (card_position(pointer, (w, h), area, px(POINTER_GAP)), area)
    });
    (x, y, w, h)
}

/// Windows: size, place and show the card on top in one SetWindowPos (physical pixels, as the
/// toast does), give it the keyboard (Esc, Enter), then nudge its height so the web view repaints.
#[cfg(windows)]
fn show_card(window: &WebviewWindow, x: i32, y: i32, w: i32, h: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_SHOWWINDOW};
    let hwnd = match window.hwnd() {
        Ok(hwnd) => hwnd,
        Err(e) => {
            applog::error(&format!("translate: no window handle: {e}"));
            return;
        }
    };
    if unsafe { SetWindowPos(hwnd.0 as _, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW | SWP_FRAMECHANGED) } == 0 {
        applog::error("translate: could not show the card");
        return;
    }
    if let Err(e) = window.set_focus() {
        applog::warn(&format!("translate: could not give the card the keyboard: {e}"));
    }
    let window = window.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(60));
        let target = window.clone();
        let _ = window.run_on_main_thread(move || nudge(&target, w, h));
    });
}

/// Windows: resize to `w` x `h` + 1 and back so the shown web view paints (see screenshot/toast.rs).
#[cfg(windows)]
fn nudge(window: &WebviewWindow, w: i32, h: i32) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER};
    let Ok(handle) = window.hwnd() else { return };
    let flags = SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE;
    unsafe {
        SetWindowPos(handle.0 as _, std::ptr::null_mut(), 0, 0, w, h + 1, flags);
        SetWindowPos(handle.0 as _, std::ptr::null_mut(), 0, 0, w, h, flags);
    }
}

/// macOS: everything in points (Cocoa's global coordinates), so mixed Retina and non-Retina
/// screens need no scale maths: the screen under the pointer (NSEvent.mouseLocation), its visible
/// frame (without menu bar and Dock), then the card near the pointer inside it (first show of a
/// card session; later shows only resize). Shown with orderFrontRegardless, which brings the
/// window up without activating HL Desk: the keyboard stays in the app the text came from (a click
/// on the card gives it the keyboard for Esc and Enter). Returns the top-left corner in points.
#[cfg(target_os = "macos")]
fn show_card_mac(window: &WebviewWindow, height: f64) -> Result<(i32, i32), String> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWindow};
    use tauri::{LogicalPosition, LogicalSize};

    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let size = (CARD_W.round() as i32, height.round() as i32);
    let mut failed = None;
    let (x, y) = session_position(size, || match pointer_place_mac(mtm, size) {
        Ok(place) => place,
        Err(e) => {
            failed = Some(e);
            ((40, 40), (0, 0, 1280, 800))
        }
    });
    if let Some(e) = failed {
        applog::warn(&format!("translate: no pointer screen ({e}); card goes to the corner"));
    }
    window.set_size(LogicalSize::new(CARD_W, height)).map_err(|e| e.to_string())?;
    window.set_position(LogicalPosition::new(x as f64, y as f64)).map_err(|e| e.to_string())?;
    let ptr = window.ns_window().map_err(|e| e.to_string())?;
    // A hidden app shows no window: unhide it (without taking the focus) first, as a safety net.
    NSApplication::sharedApplication(mtm).unhideWithoutActivation();
    // SAFETY: Tauri's NSWindow pointer for this window, used on the main thread.
    unsafe {
        let ns_window: &NSWindow = &*(ptr as *const NSWindow);
        ns_window.orderFrontRegardless();
    }
    Ok((x, y))
}

/// macOS: the card's corner near the pointer and the visible frame of the screen under it, both
/// in points with a top-left origin.
#[cfg(target_os = "macos")]
fn pointer_place_mac(
    mtm: objc2::MainThreadMarker,
    size: (i32, i32),
) -> Result<((i32, i32), (i32, i32, i32, i32)), String> {
    use objc2_app_kit::{NSEvent, NSScreen};
    // SAFETY: a class method reading the current pointer location.
    let mouse = unsafe { NSEvent::mouseLocation() };
    let screens = NSScreen::screens(mtm).to_vec();
    let rect = |s: &NSScreen| {
        let f = s.frame();
        (f.origin.x, f.origin.y, f.size.width, f.size.height)
    };
    // Cocoa's origin is the bottom-left of the first screen; Tauri's is its top-left.
    let primary_h = screens.first().map(|s| rect(&**s).3).ok_or("no screen")?;
    let screen = screens
        .iter()
        .find(|s| {
            let (x, y, w, h) = rect(&***s);
            mouse.x >= x && mouse.x < x + w && mouse.y >= y && mouse.y <= y + h
        })
        .or(screens.first())
        .ok_or("no screen")?;
    let vf = screen.visibleFrame();
    let area = (
        vf.origin.x.round() as i32,
        (primary_h - vf.origin.y - vf.size.height).round() as i32,
        vf.size.width.round() as i32,
        vf.size.height.round() as i32,
    );
    let pointer = (mouse.x.round() as i32, (primary_h - mouse.y).round() as i32);
    Ok((card_position(pointer, size, area, POINTER_GAP as i32), area))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn show_card(window: &WebviewWindow, x: i32, y: i32, w: i32, h: i32) {
    use tauri::{PhysicalPosition, PhysicalSize};
    let _ = window.set_size(PhysicalSize::new(w as u32, h as u32));
    let _ = window.set_position(PhysicalPosition::new(x, y));
    if let Err(e) = window.show() {
        applog::error(&format!("translate: could not show the card: {e}"));
        return;
    }
    if let Err(e) = window.set_focus() {
        applog::warn(&format!("translate: could not give the card the keyboard: {e}"));
    }
}

// ---------- commands ----------

/// The copy waiting for the card (None if the page already took it).
#[tauri::command]
fn translate_card_take() -> Option<CardRequest> {
    lock(&PENDING).take()
}

/// The card page has drawn and its content is `height` points tall: place it near the pointer
/// and show it (again, if it is already shown and only its height changed).
#[tauri::command]
fn translate_card_show(window: WebviewWindow, height: f64) {
    PAGE_READY.store(true, Ordering::SeqCst);
    let h = if height.is_finite() && height >= 80.0 { height.min(CARD_MAX_H) } else { CARD_H };
    #[cfg(target_os = "macos")]
    match show_card_mac(&window, h) {
        Ok((x, y)) => applog::info(&format!("translate: card shown at x={x} y={y} pt, {CARD_W}x{h} pt")),
        Err(e) => applog::error(&format!("translate: could not show the card: {e}")),
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (x, y, pw, ph) = placement(window.app_handle(), h);
        show_card(&window, x, y, pw, ph);
        applog::info(&format!("translate: card shown at x={x} y={y}, {pw}x{ph} px"));
    }
}

/// Esc, close, Copy, a click elsewhere or the next single copy: hides only the card window (never
/// the app, which would also keep the next card from showing on macOS) and ends the card session.
#[tauri::command]
fn translate_card_hide(window: WebviewWindow) {
    *lock(&SESSION) = None;
    let _ = window.hide();
    applog::info("translate: card hidden");
}

/// Copy: put the translation on the clipboard. The double-copy detector ignores it.
#[tauri::command]
fn translate_copy(app: AppHandle, text: String) -> Result<(), String> {
    *lock(&OWN) = Some(Seen { hash: hash(&text), at_ms: now_ms() });
    if let Err(e) = app.clipboard().write_text(text.clone()) {
        *lock(&OWN) = None;
        applog::error(&format!("translate: could not copy the translation: {e}"));
        return Err(format!("Could not copy: {e}"));
    }
    applog::info(&format!("translate: translation copied ({} chars)", text.chars().count()));
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Detected {
    pair: &'static str,
    script: &'static str,
}

/// Auto-detect for a text that did not come from a double copy (the screenshot editor's OCR
/// lines): the same rules and remembered picks as the card. None: no letters to translate.
#[tauri::command]
fn translate_detect(app: AppHandle, text: String) -> Option<Detected> {
    let (script, settled) = detect(&text)?;
    let pick = settings::translate_pick(&app, script.id());
    let pair = choose_pair(script, settled, pick.as_deref());
    applog::info(&format!("translate: detected {} text, pair {}", script.id(), pair.id()));
    Some(Detected { pair: pair.id(), script: script.id() })
}

/// The user picked a pair by hand for text in `script` ("han" or "latin"): remember it.
#[tauri::command]
fn translate_set_pick(app: AppHandle, script: String, pair: String) -> Result<(), String> {
    let script = Script::from_id(&script).ok_or("unknown script")?;
    let pair = Pair::from_id(&pair).ok_or("unknown language pair")?;
    applog::info(&format!("translate: picked {} for {} text", pair.id(), script.id()));
    settings::set_translate_pick(&app, script.id(), pair.id())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileView {
    kind: &'static str,
    name: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairStatus {
    pair: &'static str,
    installed: bool,
    total_bytes: u64,
    files: Vec<FileView>,
}

#[tauri::command]
fn translate_status(app: AppHandle, pair: String) -> Result<PairStatus, String> {
    let pair = Pair::from_id(&pair).ok_or("unknown language pair")?;
    let models = models_for(pair);
    let installed = installed(&pair_dir(&app, pair)?, models);
    applog::info(&format!("translate: {} models {}", pair.id(), if installed { "present" } else { "missing" }));
    Ok(PairStatus {
        pair: pair.id(),
        installed,
        total_bytes: models.total_size(),
        files: models.files.iter().map(|f| FileView { kind: f.kind, name: f.name }).collect(),
    })
}

/// Downloads the pair's model files, reporting progress on `on_progress`. If that pair is already
/// downloading, joins it instead (progress on this channel too, same result).
#[tauri::command]
async fn translate_download(app: AppHandle, pair: String, on_progress: Channel<Progress>) -> Result<(), String> {
    let pair = Pair::from_id(&pair).ok_or("unknown language pair")?;
    let (download, joined) = {
        let mut downloads = lock(&DOWNLOADS);
        match downloads.iter().find(|(p, _)| *p == pair) {
            Some((_, d)) => {
                let _ = on_progress.send(lock(&d.last).clone());
                lock(&d.listeners).push(on_progress);
                (d.clone(), true)
            }
            None => {
                let total = models_for(pair).total_size();
                let d = std::sync::Arc::new(Download {
                    listeners: Mutex::new(vec![on_progress]),
                    last: Mutex::new(Progress { done: 0, total }),
                    result: tokio::sync::watch::channel(None).0,
                });
                downloads.push((pair, d.clone()));
                (d, false)
            }
        }
    };
    if joined {
        applog::info(&format!("translate: joined the running {} download", pair.id()));
        let mut rx = download.result.subscribe();
        let result = match rx.wait_for(|r| r.is_some()).await {
            Ok(r) => r.clone().unwrap_or(Ok(())),
            Err(_) => Err("Download failed: it stopped unexpectedly".to_string()),
        };
        return result;
    }
    let result = run_download(&app, pair, &download).await;
    lock(&DOWNLOADS).retain(|(p, _)| *p != pair);
    download.result.send_replace(Some(result.clone()));
    result
}

async fn run_download(app: &AppHandle, pair: Pair, download: &Download) -> Result<(), String> {
    applog::info(&format!(
        "translate: downloading {} models v{} ({} bytes) from {MODEL_BASE_URL}",
        pair.id(),
        models_for(pair).version,
        models_for(pair).total_size()
    ));
    match download_pair(app, pair, &|p| download.report(p)).await {
        Ok(()) => {
            applog::info(&format!("translate: {} models downloaded", pair.id()));
            Ok(())
        }
        Err(e) => {
            applog::error(&format!("translate: {} download failed: {e}", pair.id()));
            Err(format!("Download failed: {e}"))
        }
    }
}

/// One model file's bytes, for the engine worker.
#[tauri::command]
async fn translate_model_file(app: AppHandle, pair: String, kind: String) -> Result<Response, String> {
    let pair = Pair::from_id(&pair).ok_or("unknown language pair")?;
    let file = models_for(pair)
        .files
        .iter()
        .find(|f| f.kind == kind)
        .ok_or("unknown model file")?;
    let path = pair_dir(&app, pair)?.join(file.name);
    let bytes = fs::read(&path).map_err(|e| {
        applog::error(&format!("translate: could not read {}: {e}", path.display()));
        format!("Could not read the language file {}: {e}", file.name)
    })?;
    if bytes.len() as u64 != file.size {
        let _ = fs::remove_file(&path);
        applog::error(&format!("translate: {} has the wrong size; deleted", file.name));
        return Err(format!("The language file {} was damaged; download it again.", file.name));
    }
    Ok(Response::new(bytes))
}

pub fn commands() -> impl Fn(Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        translate_card_take,
        translate_card_show,
        translate_card_hide,
        translate_copy,
        translate_detect,
        translate_set_pick,
        translate_status,
        translate_download,
        translate_model_file
    ]
}

/// Called once at start-up. Windows: starts the clipboard listener (see the module comment).
pub fn init(app: &AppHandle) {
    #[cfg(windows)]
    listener::start(app);
    #[cfg(not(windows))]
    let _ = app;
}

// ---------- Windows clipboard listener ----------

#[cfg(windows)]
mod listener {
    use std::{sync::OnceLock, time::Duration};

    use tauri::AppHandle;
    use tauri_plugin_clipboard_manager::ClipboardExt;
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            DataExchange::{AddClipboardFormatListener, IsClipboardFormatAvailable, RegisterClipboardFormatW},
            LibraryLoader::GetModuleHandleW,
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, RegisterClassW, HWND_MESSAGE, MSG,
            WM_CLIPBOARDUPDATE, WNDCLASSW,
        },
    };

    use crate::applog;

    static APP: OnceLock<AppHandle> = OnceLock::new();
    const CF_UNICODETEXT: u32 = 13;
    /// Formats that mark a copy private (password managers); such copies are left alone.
    const PRIVATE_FORMATS: [&str; 2] = ["ExcludeClipboardContentFromMonitorProcessing", "Clipboard Viewer Ignore"];

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn private_copy() -> bool {
        PRIVATE_FORMATS.iter().any(|name| {
            let name = wide(name);
            // SAFETY: plain Win32 calls with a NUL-terminated UTF-16 string.
            let format = unsafe { RegisterClipboardFormatW(name.as_ptr()) };
            format != 0 && unsafe { IsClipboardFormatAvailable(format) } != 0
        })
    }

    fn on_update(app: &AppHandle) {
        // SAFETY: IsClipboardFormatAvailable needs no open clipboard.
        if unsafe { IsClipboardFormatAvailable(CF_UNICODETEXT) } == 0 || private_copy() {
            super::other_copy(app);
            return;
        }
        // clipboard.rs may be reading at the same moment: try a few times.
        for _ in 0..4 {
            if let Ok(text) = app.clipboard().read_text() {
                super::handle_copy(app, &text);
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        applog::warn("translate: could not read a copied text");
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_CLIPBOARDUPDATE {
            if let Some(app) = APP.get() {
                on_update(app);
            }
            return 0;
        }
        DefWindowProcW(hwnd, msg, wparam, lparam)
    }

    /// A message-only window on its own thread, registered as a clipboard format listener.
    pub fn start(app: &AppHandle) {
        if APP.set(app.clone()).is_err() {
            return;
        }
        std::thread::spawn(|| {
            let class = wide("HLDeskTranslateClipboard");
            // SAFETY: standard window-class registration and message loop on this thread; the
            // class name outlives the window (this function never returns while it runs).
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let mut wc: WNDCLASSW = std::mem::zeroed();
                wc.lpfnWndProc = Some(wndproc);
                wc.hInstance = instance;
                wc.lpszClassName = class.as_ptr();
                if RegisterClassW(&wc) == 0 {
                    applog::error("translate: could not register the clipboard listener window");
                    return;
                }
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    std::ptr::null(),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                if hwnd.is_null() {
                    applog::error("translate: could not create the clipboard listener window");
                    return;
                }
                if AddClipboardFormatListener(hwnd) == 0 {
                    applog::error("translate: could not listen to the clipboard");
                    return;
                }
                applog::info("translate: listening for double copies");
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    DispatchMessageW(&msg);
                }
            }
        });
    }
}

// ---------- tests ----------

#[cfg(test)]
mod tests {
    use super::*;

    fn seen(text: &str, at_ms: u64) -> Seen {
        Seen { hash: hash(text), at_ms }
    }

    #[test]
    fn double_copy_same_text_in_window() {
        assert!(is_double_copy(Some(seen("hello", 1_000)), seen("hello", 1_250)));
        assert!(is_double_copy(Some(seen("hello", 1_000)), seen("hello", 1_000 + DOUBLE_COPY_MS)));
        // macOS: two quick copies seen on two consecutive 100 ms looks.
        assert!(is_double_copy(Some(seen("hello", 1_000)), seen("hello", 1_100)));
    }

    #[test]
    fn not_double_copy() {
        assert!(!is_double_copy(None, seen("hello", 1_000)));
        assert!(!is_double_copy(Some(seen("hello", 1_000)), seen("other", 1_200)));
        assert!(!is_double_copy(Some(seen("hello", 1_000)), seen("hello", 1_000 + DOUBLE_COPY_MS + 1)));
        // One copy written twice by the app within a few ms.
        assert!(!is_double_copy(Some(seen("hello", 1_000)), seen("hello", 1_005)));
        // Clock went backwards.
        assert!(!is_double_copy(Some(seen("hello", 2_000)), seen("hello", 1_900)));
    }

    #[test]
    fn card_keeps_its_place_when_resized() {
        let area = (0, 0, 1920, 1040);
        assert_eq!(keep_inside((112, 112), (360, 300), area), (112, 112));
        // Grew past the bottom: pulled up just enough.
        assert_eq!(keep_inside((112, 900), (360, 300), area), (112, 740));
        // Second screen to the left.
        assert_eq!(keep_inside((-500, 100), (360, 200), (-1920, 0, 1920, 1080)), (-500, 100));
    }

    #[test]
    fn detects_traditional_chinese() {
        assert_eq!(detect("貨物星期五前可以出，運費另計，請確認數量。"), Some((Script::Han, None)));
        // Mixed with Latin letters and numbers, still Chinese.
        assert_eq!(
            detect("報價單 Q-2311，產品：膠盒 x 500，單價：HK$12.5，交貨期：7 個工作天。"),
            Some((Script::Han, None))
        );
    }

    #[test]
    fn detects_simplified_chinese() {
        assert_eq!(detect("我们明天下午三点开会，请准备好样品。"), Some((Script::Han, Some(Pair::ZhHansEn))));
        assert_eq!(detect("这个"), Some((Script::Han, Some(Pair::ZhHansEn))));
    }

    #[test]
    fn detects_english() {
        assert_eq!(
            detect("Please confirm the delivery date and send the invoice by Friday."),
            Some((Script::Latin, Some(Pair::EnZhHant)))
        );
        // One Chinese word in an English sentence stays English.
        assert_eq!(detect("Send it to the 深圳 office tomorrow please"), Some((Script::Latin, Some(Pair::EnZhHant))));
    }

    #[test]
    fn detects_nothing() {
        assert_eq!(detect("12345 !!! ---"), None);
        assert_eq!(detect(""), None);
    }

    #[test]
    fn simplified_list_is_han_and_has_no_traditional_forms() {
        assert!(SIMPLIFIED_ONLY.chars().all(is_han));
        for c in "們這個說時國會對來發經過還進動樣現學問為門車東長見書買賣讀語話請讓記".chars() {
            assert!(!SIMPLIFIED_ONLY.contains(c), "{c} is Traditional");
        }
    }

    #[test]
    fn chooses_pair() {
        // Settled by the text.
        assert_eq!(choose_pair(Script::Han, Some(Pair::ZhHansEn), Some("zh-Hant-en")), Pair::ZhHansEn);
        // Ambiguous Han text: last pick, else Traditional.
        assert_eq!(choose_pair(Script::Han, None, Some("zh-Hans-en")), Pair::ZhHansEn);
        assert_eq!(choose_pair(Script::Han, None, None), Pair::ZhHantEn);
        // A pick from the other script is ignored.
        assert_eq!(choose_pair(Script::Han, None, Some("en-zh-Hant")), Pair::ZhHantEn);
        assert_eq!(choose_pair(Script::Latin, Some(Pair::EnZhHant), Some("zh-Hant-en")), Pair::EnZhHant);
        assert_eq!(choose_pair(Script::Latin, Some(Pair::EnZhHant), Some("bogus")), Pair::EnZhHant);
    }

    #[test]
    fn pair_ids_round_trip() {
        for p in Pair::ALL {
            assert_eq!(Pair::from_id(p.id()), Some(p));
        }
        assert_eq!(Pair::from_id("fr-en"), None);
    }

    #[test]
    fn manifest_is_complete() {
        for p in Pair::ALL {
            let m = models_for(p);
            assert!(!m.version.is_empty());
            let kinds: Vec<&str> = m.files.iter().map(|f| f.kind).collect();
            assert!(kinds.contains(&"model") && kinds.contains(&"lex"), "{}", p.id());
            assert!(
                kinds.contains(&"vocab") || (kinds.contains(&"srcvocab") && kinds.contains(&"trgvocab")),
                "{}",
                p.id()
            );
            for f in m.files {
                assert_eq!(f.sha256.len(), 64, "{}", f.name);
                assert!(f.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
                assert!(f.size > 0);
                assert!(!f.name.contains(['/', '\\']));
            }
            // About 50 to 70 MB per pair.
            assert!((40_000_000..80_000_000).contains(&m.total_size()), "{}", p.id());
        }
        assert!(MODEL_BASE_URL.starts_with("https://") && MODEL_BASE_URL.ends_with('/'));
    }

    #[test]
    fn model_url_on_the_mirror() {
        let model = models_for(Pair::ZhHantEn).files.iter().find(|f| f.kind == "model").unwrap();
        assert_eq!(
            model_url(Pair::ZhHantEn, model),
            "https://github.com/ops612/hl-desk-releases/releases/download/models-v1/zh-Hant-en.model.zhen.intgemm.alphas.bin"
        );
    }

    #[test]
    fn hash_check() {
        let dir = std::env::temp_dir().join(format!("hl-desk-translate-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("abc.bin");
        fs::write(&path, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_file(&path).unwrap(), abc);
        assert!(verify_file(&path, 3, abc).is_ok());
        assert!(verify_file(&path, 4, abc).is_err());
        assert!(verify_file(&path, 3, &"0".repeat(64)).is_err());
        assert!(verify_file(&dir.join("missing.bin"), 3, abc).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn card_goes_near_the_pointer() {
        let area = (0, 0, 1920, 1040);
        // Below and right of the pointer.
        assert_eq!(card_position((100, 100), (360, 200), area, 12), (112, 112));
        // No room below: above the pointer.
        assert_eq!(card_position((100, 1000), (360, 200), area, 12), (112, 788));
        // No room right: pulled back inside.
        assert_eq!(card_position((1900, 100), (360, 200), area, 12), (1560, 112));
        // Second monitor to the left (negative x).
        assert_eq!(card_position((-500, 100), (360, 200), (-1920, 0, 1920, 1080), 12), (-488, 112));
    }
}
