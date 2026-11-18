// Translate card (translate.html), opened by src-tauri/src/translate.rs after a double copy.
// One window, reused: Rust creates it hidden, then for each double copy this page takes the text
// (translate_card_take), draws, reports its height (translate_card_show: Rust places it near the
// pointer and shows it) and translates with the shared engine (./engine.ts).
// Models are downloaded on first use of a pair, after the user clicks Download (about 50-70 MB).
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "../styles.css";
import "./translate.css";
import {
  PAIRS,
  type PairId,
  type PairStatus,
  type Script,
  copyTranslation,
  describe,
  downloadPair,
  downloadTexts,
  log,
  pairInfo,
  pairStatus,
  rememberPick,
  touchIdle,
  translateTexts,
} from "./engine";

interface CardRequest {
  text: string;
  pair: PairId;
  script: Script;
  tooLong: boolean;
}

window.addEventListener("error", (e) => log("error", `script error: ${describe(e.error ?? e.message)}`));
window.addEventListener("unhandledrejection", (e) => log("error", `unhandled promise: ${describe(e.reason)}`));

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const card = $("card");
const sourceEl = $("source");
const resultEl = $("result");
const getEl = $("get");
const getTitle = $("get-title");
const getText = $("get-text");
const progress = $("progress");
const progressBar = $("progress-bar");
const getStatus = $("get-status");
const errorEl = $("error");
const langBtn = $<HTMLButtonElement>("lang");
const langLabel = $("lang-label");
const menu = $("menu");
const primary = $<HTMLButtonElement>("primary");
const pinBtn = $<HTMLButtonElement>("pin");

// ---------- card ----------

let request: CardRequest | null = null;
let pair: PairId = "zh-Hant-en";
let translation = "";
let token = 0;
let mode: "idle" | "translating" | "done" | "offer" | "downloading" | "failed" = "idle";
let retry: (() => void) | null = null;
let pinned = false;
let shownAt = 0;
/** Between a new double copy (open) and hide: only then may the card be shown. */
let visible = false;
let fontsReady: Promise<unknown> | null = null;

function setPrimary(label: string, enabled: boolean): void {
  primary.textContent = label;
  primary.disabled = !enabled;
}

function showError(message: string, again: (() => void) | null): void {
  errorEl.textContent = message;
  errorEl.hidden = false;
  retry = again;
  mode = "failed";
  setPrimary(again ? "Retry" : "Copy", !!again);
  void report();
}

function clearMessages(): void {
  errorEl.hidden = true;
  errorEl.textContent = "";
  getEl.hidden = true;
  retry = null;
}

/** Rust sizes and places the window to the card's height (points) and shows it. */
async function report(): Promise<void> {
  if (!visible) return;
  if (!fontsReady) fontsReady = document.fonts.ready.catch(() => {});
  await fontsReady;
  await new Promise((r) => requestAnimationFrame(r));
  // Hidden meanwhile (Esc, a single copy): a late result must never show the card again.
  if (!visible) return;
  const border = document.body.offsetHeight - document.body.clientHeight;
  const height = Math.ceil(card.getBoundingClientRect().height + border);
  await invoke("translate_card_show", { height }).catch((e) => log("error", `show: ${describe(e)}`));
  if (!shownAt) {
    shownAt = Date.now();
    // The content (not the window) fades in from 0.98 each time the card appears (translate.css).
    card.classList.remove("tr-in");
    void card.offsetWidth;
    card.classList.add("tr-in");
  }
}

function hide(): void {
  closeMenu();
  // Ends this card's work: a translation, download step or error that finishes later is ignored.
  visible = false;
  token++;
  shownAt = 0;
  void invoke("translate_card_hide").catch((e) => log("error", `hide: ${describe(e)}`));
  touchIdle();
}

function renderLang(): void {
  langLabel.textContent = pairInfo(pair).label;
  langBtn.title = `Language: ${pairInfo(pair).label}`;
}

async function run(): Promise<void> {
  const req = request;
  if (!req) return;
  const mine = ++token;
  clearMessages();
  translation = "";
  renderLang();
  if (req.tooLong) {
    resultEl.textContent = "";
    showError("This text is too long to translate here (over 5,000 characters).", null);
    return;
  }
  let status: PairStatus;
  try {
    status = await pairStatus(pair);
  } catch (e) {
    showError(`Could not check the language files: ${describe(e)}`, () => void run());
    return;
  }
  if (mine !== token) return;
  if (!status.installed) {
    offerDownload(status);
    return;
  }
  mode = "translating";
  resultEl.textContent = "Translating…";
  resultEl.classList.add("pending");
  setPrimary("Copy", false);
  void report();
  try {
    const [out] = await translateTexts(status, [req.text]);
    if (mine !== token) return;
    translation = out;
    resultEl.classList.remove("pending");
    resultEl.textContent = translation;
    mode = "done";
    setPrimary("Copy", true);
  } catch (e) {
    if (mine !== token) return;
    log("error", `translation failed (${pair}): ${describe(e)}`);
    resultEl.textContent = "";
    resultEl.classList.remove("pending");
    showError(`Could not translate: ${describe(e)}`, () => void run());
  }
  void report();
}

function offerDownload(status: PairStatus): void {
  mode = "offer";
  resultEl.textContent = "";
  resultEl.classList.remove("pending");
  getEl.hidden = false;
  const texts = downloadTexts(status);
  getTitle.textContent = texts.title;
  getText.textContent = texts.text;
  progress.hidden = true;
  getStatus.hidden = true;
  setPrimary("Download", true);
  retry = () => void download(status);
  void report();
}

async function download(status: PairStatus): Promise<void> {
  const mine = token;
  mode = "downloading";
  errorEl.hidden = true;
  getEl.hidden = false;
  progress.hidden = false;
  getStatus.hidden = false;
  setPrimary("Download", false);
  const texts = downloadTexts(status);
  const show = (done: number) => {
    progressBar.style.width = `${Math.min(100, (done / status.totalBytes) * 100)}%`;
    getStatus.textContent = texts.progress(done);
  };
  show(0);
  void report();
  try {
    await downloadPair(status, show);
    if (mine === token && pair === status.pair) await run();
  } catch (e) {
    log("error", `download of ${status.pair} failed: ${describe(e)}`);
    if (mine !== token) return;
    progress.hidden = true;
    getStatus.hidden = true;
    showError(describe(e), () => void download(status));
  }
}

async function copy(): Promise<void> {
  if (mode !== "done" || !translation) return;
  try {
    await copyTranslation(translation);
    setPrimary("Copied", false);
    window.setTimeout(() => {
      if (mode === "done") setPrimary("Copy", true);
      if (!pinned) hide();
    }, 600);
  } catch (e) {
    showError(describe(e), () => void copy());
  }
}

function onPrimary(): void {
  if (mode === "done") void copy();
  else if (retry) retry();
}

// ---------- language picker ----------

function closeMenu(): void {
  menu.hidden = true;
  langBtn.setAttribute("aria-expanded", "false");
}

function openMenu(): void {
  menu.replaceChildren(
    ...PAIRS.map((p) => {
      const item = document.createElement("button");
      item.type = "button";
      item.className = "row";
      item.setAttribute("role", "menuitemradio");
      item.setAttribute("aria-checked", String(p.id === pair));
      item.textContent = p.label;
      item.addEventListener("click", () => pick(p.id));
      return item;
    }),
  );
  menu.hidden = false;
  langBtn.setAttribute("aria-expanded", "true");
  (menu.querySelector('[aria-checked="true"]') as HTMLElement | null)?.focus();
}

function pick(id: PairId): void {
  closeMenu();
  if (id === pair || !request) return;
  pair = id;
  log("info", `picked ${id} by hand`);
  rememberPick(request.script, id);
  void run();
}

langBtn.addEventListener("click", () => (menu.hidden ? openMenu() : closeMenu()));
menu.addEventListener("keydown", (e) => {
  const items = [...menu.querySelectorAll<HTMLElement>(".row")];
  const i = items.indexOf(document.activeElement as HTMLElement);
  if (e.key === "ArrowDown" || e.key === "ArrowUp") {
    e.preventDefault();
    items[(i + (e.key === "ArrowDown" ? 1 : items.length - 1)) % items.length]?.focus();
  } else if (e.key === "Escape") {
    e.preventDefault();
    e.stopPropagation();
    closeMenu();
    langBtn.focus();
  }
});
document.addEventListener("pointerdown", (e) => {
  if (!menu.hidden && !(e.target as Element).closest(".tr-picker")) closeMenu();
});

// ---------- wiring ----------

primary.addEventListener("click", onPrimary);
$("close").addEventListener("click", hide);
pinBtn.addEventListener("click", () => {
  pinned = !pinned;
  pinBtn.setAttribute("aria-pressed", String(pinned));
  log("info", pinned ? "pinned" : "unpinned");
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    hide();
  } else if (e.key === "Enter" && menu.hidden && !(e.target instanceof HTMLButtonElement && e.target !== primary)) {
    e.preventDefault();
    onPrimary();
  }
});
// A click elsewhere hides the card unless it is pinned (only once the card had the keyboard; on
// macOS it comes up without it).
window.addEventListener("blur", () => {
  if (!pinned && shownAt && Date.now() - shownAt > 400) hide();
});

/** A new double copy: take its text and start. */
async function open(): Promise<void> {
  const req = await invoke<CardRequest | null>("translate_card_take").catch((e) => {
    log("error", `take: ${describe(e)}`);
    return null;
  });
  if (!req) return;
  request = req;
  pair = req.pair;
  visible = true;
  closeMenu();
  sourceEl.textContent = req.text.length > 600 ? `${req.text.slice(0, 600)}…` : req.text;
  sourceEl.scrollTop = 0;
  shownAt = 0;
  log("info", `card opened (${req.script}, ${req.pair}, ${req.text.length} chars)`);
  await run();
}

// macOS: the card comes up without the keyboard (the user's app keeps it), so Esc and Enter
// work only after a click on the card.
if (/Mac/.test(navigator.userAgent)) {
  document.querySelector(".tr-footer")!.textContent = "Offline · click the card for Esc / Enter";
}

void listen("translate://open", () => void open());
// A single copy elsewhere: an unpinned card hides (Rust sends it only while the card is shown).
void listen("translate://copied", () => {
  if (!pinned) hide();
});
log("info", "page loaded");
void open();
