// Translate window (translator.html, label "translator"), opened by src-tauri/src/translator.rs
// with Ctrl+Shift+1 (Cmd+Shift+1 on macOS) or the main window's Translate button. Staff type or
// paste text and get a live offline translation to copy into WhatsApp. It translates about 500 ms
// after typing stops, with the shared engine (src/translate/engine.ts) and the same detection and
// first-use download as the translate card. Pure parts (pickers, debounce, latest wins, voice):
// ./logic.ts. Opens empty after it was hidden; a shortcut press while it is shown only focuses it.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "../styles.css";
import "../translate/translate.css";
import "./translator.css";
import {
  type PairId,
  type PairStatus,
  copyTranslation,
  describe,
  detectPair,
  downloadPair,
  downloadTexts,
  log as engineLog,
  pairStatus,
  touchIdle,
  translateTexts,
} from "../translate/engine";
import {
  LABELS,
  Latest,
  SOURCES,
  TARGETS,
  type Picks,
  type Source,
  type Target,
  debounce,
  pickSource,
  pickTarget,
  pickVoice,
  resolve,
  sourceOf,
  swap,
  targetOf,
} from "./logic";

const log = (level: "info" | "warn" | "error", message: string) => engineLog(level, `window: ${message}`);
window.addEventListener("error", (e) => log("error", `script error: ${describe(e.error ?? e.message)}`));
window.addEventListener("unhandledrejection", (e) => log("error", `unhandled promise: ${describe(e.reason)}`));

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const textEl = $<HTMLTextAreaElement>("text");
const chip = $("detected");
const sourceSel = $<HTMLSelectElement>("source");
const targetSel = $<HTMLSelectElement>("target");
const resultEl = $("result");
const getEl = $("get");
const getTitle = $("get-title");
const getText = $("get-text");
const progress = $("progress");
const progressBar = $("progress-bar");
const getStatus = $("get-status");
const errorEl = $("error");
const primary = $<HTMLButtonElement>("primary");
const speakBtn = $<HTMLButtonElement>("speak");
const pinBtn = $<HTMLButtonElement>("pin");

const isMac = navigator.userAgent.includes("Mac");
const MAX_CHARS = 5_000; // as translate.rs MAX_CHARS
const DEBOUNCE_MS = 500;

let picks: Picks = { source: "auto", target: "en" };
/** The pair auto-detect chose for the current text (null: no text or no letters). */
let detected: PairId | null = null;
/** The pair of the shown translation. */
let pair: PairId | null = null;
let translation = "";
let mode: "empty" | "translating" | "done" | "offer" | "downloading" | "failed" = "empty";
let retry: (() => void) | null = null;
let copiedTimer = 0;
const latest = new Latest();
/** Download progress (bytes done) per pair being downloaded: typing on does not restart it. */
const downloading = new Map<PairId, number>();

// ---------- pickers ----------

function fillSelect<T extends string>(sel: HTMLSelectElement, values: readonly T[]): void {
  sel.replaceChildren(
    ...values.map((v) => {
      const o = document.createElement("option");
      o.value = v;
      o.textContent = LABELS[v as Source];
      return o;
    }),
  );
}

function renderPickers(): void {
  sourceSel.value = picks.source;
  // Auto: the target shows where the detected text goes.
  targetSel.value = picks.source === "auto" && detected ? targetOf(detected) : picks.target;
  const show = picks.source === "auto" && detected !== null;
  chip.hidden = !show;
  if (show) chip.textContent = `Detected ${LABELS[sourceOf(detected!)]}`;
}

function setPicks(next: Picks, why: string): void {
  picks = next;
  log("info", `${why}: ${picks.source} to ${picks.target}`);
  renderPickers();
  typing.cancel();
  void run();
}

// ---------- result ----------

function setPrimary(label: string, enabled: boolean): void {
  clearTimeout(copiedTimer);
  primary.textContent = label;
  primary.disabled = !enabled;
}

function clearMessages(): void {
  errorEl.hidden = true;
  errorEl.textContent = "";
  getEl.hidden = true;
  retry = null;
}

function showResult(text: string, pending = false): void {
  resultEl.textContent = text;
  resultEl.classList.toggle("pending", pending);
}

function showError(message: string, again: (() => void) | null): void {
  errorEl.textContent = message;
  errorEl.hidden = false;
  retry = again;
  mode = "failed";
  setPrimary(again ? "Retry" : "Copy", !!again);
  renderSpeak();
}

/** Translates the text box now (typing calls it through the debounce). */
async function run(): Promise<void> {
  const current = latest.begin();
  const text = textEl.value;
  clearMessages();
  if (!text.trim()) {
    detected = null;
    pair = null;
    translation = "";
    mode = "empty";
    showResult("");
    setPrimary("Copy", false);
    renderPickers();
    renderSpeak();
    return;
  }
  if (text.length > MAX_CHARS) {
    translation = "";
    showResult("");
    showError("This text is too long to translate here (over 5,000 characters).", null);
    return;
  }
  if (picks.source === "auto") {
    const found = await detectPair(text).catch((e) => {
      log("error", `detect: ${describe(e)}`);
      return null;
    });
    if (!current()) return;
    detected = found?.pair ?? null;
  }
  renderPickers();
  const next = resolve(picks, detected);
  if (!next) {
    translation = "";
    mode = "empty";
    showResult("No Chinese or English text to translate.", true);
    setPrimary("Copy", false);
    renderSpeak();
    return;
  }
  let status: PairStatus;
  try {
    status = await pairStatus(next);
  } catch (e) {
    if (!current()) return;
    showError(`Could not check the language files: ${describe(e)}`, () => void run());
    return;
  }
  if (!current()) return;
  if (!status.installed) {
    translation = "";
    showResult("");
    showDownload(status);
    return;
  }
  // The last translation stays (muted) while the new one is made, so typing does not flicker.
  mode = "translating";
  if (pair === next && translation) showResult(translation, true);
  else showResult("Translating…", true);
  setPrimary("Copy", false);
  renderSpeak();
  try {
    const [out] = await translateTexts(status, [text]);
    if (!current()) return;
    pair = next;
    translation = out;
    showResult(translation);
    mode = "done";
    setPrimary("Copy", true);
  } catch (e) {
    if (!current()) return;
    log("error", `translation failed (${next}): ${describe(e)}`);
    translation = "";
    showResult("");
    showError(`Could not translate: ${describe(e)}`, () => void run());
  }
  renderSpeak();
}

const typing = debounce(() => void run(), DEBOUNCE_MS);

// ---------- first-use download (same wording as the card) ----------

function showDownload(status: PairStatus): void {
  const texts = downloadTexts(status);
  getEl.hidden = false;
  getTitle.textContent = texts.title;
  getText.textContent = texts.text;
  renderSpeak();
  const done = downloading.get(status.pair);
  if (done === undefined) {
    mode = "offer";
    progress.hidden = true;
    getStatus.hidden = true;
    setPrimary("Download", true);
    retry = () => void download(status);
  } else {
    mode = "downloading";
    showProgress(status, done);
  }
}

function showProgress(status: PairStatus, done: number): void {
  progress.hidden = false;
  getStatus.hidden = false;
  progressBar.style.width = `${Math.min(100, (done / status.totalBytes) * 100)}%`;
  getStatus.textContent = downloadTexts(status).progress(done);
  setPrimary("Download", false);
}

async function download(status: PairStatus): Promise<void> {
  if (downloading.has(status.pair)) return;
  downloading.set(status.pair, 0);
  errorEl.hidden = true;
  mode = "downloading";
  /** Only while this pair's block is the one shown (typing may have moved on). */
  const shown = () => mode === "downloading" && !getEl.hidden && getTitle.textContent === downloadTexts(status).title;
  showProgress(status, 0);
  try {
    await downloadPair(status, (done) => {
      downloading.set(status.pair, done);
      if (shown()) showProgress(status, done);
    });
    downloading.delete(status.pair);
    if (shown()) await run();
  } catch (e) {
    downloading.delete(status.pair);
    log("error", `download of ${status.pair} failed: ${describe(e)}`);
    if (!shown()) return;
    progress.hidden = true;
    getStatus.hidden = true;
    showError(describe(e), () => void download(status));
  }
}

// ---------- copy, read aloud ----------

async function copy(): Promise<void> {
  if (mode !== "done" || !translation) return;
  try {
    // translate_copy: the double-copy listener skips this write, so the card does not pop up.
    await copyTranslation(translation);
    setPrimary("Copied", true);
    copiedTimer = window.setTimeout(() => {
      if (mode === "done") setPrimary("Copy", true);
    }, 1200);
  } catch (e) {
    showError(describe(e), () => void copy());
  }
}

function onPrimary(): void {
  if (mode === "done") void copy();
  else if (retry) retry();
}

const speech: SpeechSynthesis | null = "speechSynthesis" in window ? window.speechSynthesis : null;

/** The local voice for the shown translation's language; the icon is hidden without one. */
function voice(): SpeechSynthesisVoice | null {
  if (!speech || !pair) return null;
  return pickVoice(speech.getVoices(), targetOf(pair));
}

function renderSpeak(): void {
  speakBtn.hidden = !(mode === "done" && translation && voice());
}

function speak(): void {
  const v = voice();
  if (!speech || !v || !translation) return;
  if (speech.speaking) {
    speech.cancel();
    return;
  }
  const u = new SpeechSynthesisUtterance(translation);
  u.voice = v;
  u.lang = v.lang;
  speech.speak(u);
  log("info", `read aloud (${v.lang}, ${translation.length} chars)`);
}

// Voices arrive late in some web views.
speech?.addEventListener("voiceschanged", renderSpeak);

// ---------- window ----------

function hide(): void {
  typing.cancel();
  latest.cancel();
  speech?.cancel();
  void getCurrentWindow().hide().catch((e) => log("error", `hide: ${describe(e)}`));
  touchIdle();
}

/** A fresh open (after hiding): empty, Auto, as on the first open. */
function reset(): void {
  textEl.value = "";
  picks = { source: "auto", target: "en" };
  void run();
}

function focusText(): void {
  textEl.focus();
}

let pinned = true;
pinBtn.addEventListener("click", async () => {
  const want = !pinned;
  try {
    await getCurrentWindow().setAlwaysOnTop(want);
    pinned = want;
    pinBtn.setAttribute("aria-pressed", String(pinned));
    log("info", pinned ? "kept on top" : "no longer on top");
  } catch (e) {
    log("error", `always on top: ${describe(e)}`);
  }
});
$("close").addEventListener("click", hide);
primary.addEventListener("click", onPrimary);
speakBtn.addEventListener("click", speak);
$("swap").addEventListener("click", () => {
  // The translation becomes the text to translate back.
  if (mode === "done" && translation) textEl.value = translation;
  setPicks(swap(picks, detected), "swapped");
  focusText();
});
sourceSel.addEventListener("change", () => setPicks(pickSource(picks, sourceSel.value as Source), "source picked"));
targetSel.addEventListener("change", () => setPicks(pickTarget(picks, targetSel.value as Target), "target picked"));
textEl.addEventListener("input", () => typing.call());
document.addEventListener("keydown", (e) => {
  const cmd = isMac ? e.metaKey : e.ctrlKey;
  if (e.key === "Escape") {
    e.preventDefault();
    hide();
  } else if (e.key === "Enter" && cmd) {
    // Ctrl/Cmd+Enter copies; plain Enter is a new line in the text box.
    e.preventDefault();
    if (mode !== "done") {
      // Typed and copied at once: translate now instead of waiting for the pause.
      typing.cancel();
      void run().then(() => void copy());
    } else void copy();
  }
});

if (isMac) $("hint").textContent = "Cmd+Shift+1 open · Cmd+Enter copy result · Esc close";
fillSelect(sourceSel, SOURCES);
fillSelect(targetSel, TARGETS);
renderPickers();
// Rust: fresh = it was hidden (start empty), else it was already open (only focus).
void listen<boolean>("translator://open", (e) => {
  if (e.payload) reset();
  focusText();
});
window.addEventListener("focus", focusText, { once: true });
log("info", "page loaded");
focusText();
