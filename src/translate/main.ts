// Translate card (translate.html), opened by src-tauri/src/translate.rs after a double copy.
// One window, reused: Rust creates it hidden, then for each double copy this page takes the text
// (translate_card_take), draws, reports its height (translate_card_show: Rust places it near the
// pointer and shows it) and translates. The engine (Bergamot, public/translate/) runs in a Web
// Worker that holds one language pair; it is stopped after 2 minutes idle to give memory back
// (about 330 MB per pair once used) and when another pair is needed.
// Models are downloaded on first use of a pair, after the user clicks Download (about 50-70 MB).
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "../styles.css";
import "./translate.css";

type PairId = "zh-Hant-en" | "zh-Hans-en" | "en-zh-Hant";
interface CardRequest {
  text: string;
  pair: PairId;
  script: "han" | "latin";
  tooLong: boolean;
}
interface PairStatus {
  pair: PairId;
  installed: boolean;
  totalBytes: number;
  files: { kind: string; name: string }[];
}

const PAIRS: { id: PairId; label: string; from: string; to: string }[] = [
  { id: "zh-Hant-en", label: "Chinese (Traditional) → English", from: "zh-Hant", to: "en" },
  { id: "zh-Hans-en", label: "Chinese (Simplified) → English", from: "zh-Hans", to: "en" },
  { id: "en-zh-Hant", label: "English → Chinese (Traditional)", from: "en", to: "zh-Hant" },
];
const pairInfo = (id: PairId) => PAIRS.find((p) => p.id === id)!;
const IDLE_MS = 2 * 60 * 1000;
const MB = 1_000_000;

function log(level: "info" | "warn" | "error", message: string): void {
  void invoke("app_log", { level, message: `translate: ${message}` }).catch(() => {});
}
const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
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

// ---------- engine (Web Worker) ----------

let worker: Worker | null = null;
let workerPair: PairId | null = null;
let workerReady: Promise<void> | null = null;
let callSeq = 0;
const calls = new Map<number, { res: (v: any) => void; rej: (e: Error) => void }>();
let idleTimer = 0;

function stopEngine(reason: string): void {
  if (!worker) return;
  worker.terminate();
  log("info", `engine stopped (${reason})`);
  worker = null;
  workerPair = null;
  workerReady = null;
  for (const c of calls.values()) c.rej(new Error("The translation engine stopped."));
  calls.clear();
}

function touchIdle(): void {
  clearTimeout(idleTimer);
  idleTimer = window.setTimeout(() => stopEngine("2 minutes idle"), IDLE_MS);
}

function call<T>(msg: Record<string, unknown>, transfer: Transferable[] = []): Promise<T> {
  return new Promise((res, rej) => {
    if (!worker) return rej(new Error("The translation engine is not running."));
    const id = ++callSeq;
    calls.set(id, { res, rej });
    worker.postMessage({ ...msg, id }, transfer);
  });
}

/** A model file from Rust as an ArrayBuffer (a plain number array on Tauri's fallback IPC path). */
function toBuffer(data: unknown): ArrayBuffer {
  if (data instanceof ArrayBuffer) return data;
  if (ArrayBuffer.isView(data)) return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer;
  if (Array.isArray(data)) return Uint8Array.from(data as number[]).buffer;
  throw new Error(`unexpected file data (${Object.prototype.toString.call(data)})`);
}

/** Starts a worker for `pair` (stopping one for another pair) and loads its model. */
function ensureEngine(status: PairStatus): Promise<void> {
  if (worker && workerPair === status.pair && workerReady) return workerReady;
  stopEngine(workerPair ? `switching to ${status.pair}` : "restart");
  const w = new Worker("/translate/engine.worker.js");
  worker = w;
  workerPair = status.pair;
  w.onmessage = ({ data }) => {
    const c = calls.get(data.id);
    if (!c) return;
    calls.delete(data.id);
    if (data.logs?.length) log("info", `engine: ${data.logs.join(" | ").slice(0, 500)}`);
    if (data.ok) c.res(data.result);
    else c.rej(new Error(data.error));
  };
  w.onerror = (e) => {
    log("error", `engine worker error: ${e.message}`);
    stopEngine("worker error");
  };
  const info = pairInfo(status.pair);
  workerReady = (async () => {
    const t0 = performance.now();
    const res = await fetch("/translate/bergamot-translator.wasm");
    if (!res.ok) throw new Error(`could not read the engine (${res.status})`);
    const wasm = await res.arrayBuffer();
    const init = await call<{ ms: number; heapMB: number }>({ cmd: "init", wasm }, [wasm]);
    log("info", `engine started in ${init.ms} ms (${init.heapMB} MB)`);
    const files: Record<string, { name: string; buffer: ArrayBuffer }> = {};
    for (const f of status.files) {
      const buffer = toBuffer(await invoke("translate_model_file", { pair: status.pair, kind: f.kind }));
      files[f.kind] = { name: f.name, buffer };
    }
    const buffers = Object.values(files).map((f) => f.buffer);
    const load = await call<{ ms: number; heapMB: number }>({ cmd: "load", from: info.from, to: info.to, files }, buffers);
    log("info", `${status.pair} loaded in ${Math.round(performance.now() - t0)} ms (build ${load.ms} ms, ${load.heapMB} MB)`);
  })();
  workerReady.catch((e) => {
    log("error", `engine start failed: ${describe(e)}`);
    if (workerPair === status.pair) stopEngine("start failed");
  });
  return workerReady;
}

// ---------- text ----------

const HAN = "\\u3400-\\u4dbf\\u4e00-\\u9fff\\uf900-\\ufaff";
const ZH_PUNCT = new RegExp(`([${HAN}])\\s*([,;:?!])\\s*(?=[${HAN}])`, "g");
const FULL_WIDTH: Record<string, string> = { ",": "，", ";": "；", ":": "：", "?": "？", "!": "！" };

/** Chinese output: ASCII , ; : ? ! between two Han characters become full-width. */
function fixZhPunctuation(text: string): string {
  return text.replace(ZH_PUNCT, (_m, han: string, p: string) => han + FULL_WIDTH[p]);
}

// ---------- card ----------

let request: CardRequest | null = null;
let pair: PairId = "zh-Hant-en";
let translation = "";
let token = 0;
let mode: "idle" | "translating" | "done" | "offer" | "downloading" | "failed" = "idle";
let retry: (() => void) | null = null;
let pinned = false;
let shownAt = 0;
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
  if (!fontsReady) fontsReady = document.fonts.ready.catch(() => {});
  await fontsReady;
  await new Promise((r) => requestAnimationFrame(r));
  const border = document.body.offsetHeight - document.body.clientHeight;
  const height = Math.ceil(card.getBoundingClientRect().height + border);
  await invoke("translate_card_show", { height }).catch((e) => log("error", `show: ${describe(e)}`));
  if (!shownAt) shownAt = Date.now();
}

function hide(): void {
  closeMenu();
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
    status = await invoke<PairStatus>("translate_status", { pair });
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
    clearTimeout(idleTimer);
    await ensureEngine(status);
    if (mine !== token) return;
    const out = await call<{ text: string; ms: number; heapMB: number }>({ cmd: "translate", text: req.text });
    if (mine !== token) return;
    const info = pairInfo(pair);
    translation = info.to.startsWith("zh") ? fixZhPunctuation(out.text) : out.text;
    log("info", `translated ${req.text.length} chars with ${pair} in ${out.ms} ms (${out.heapMB} MB)`);
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
  } finally {
    touchIdle();
  }
  void report();
}

function offerDownload(status: PairStatus): void {
  mode = "offer";
  resultEl.textContent = "";
  resultEl.classList.remove("pending");
  getEl.hidden = false;
  getTitle.textContent = `Get ${pairInfo(status.pair).label}`;
  getText.textContent = `One download, about ${Math.round(status.totalBytes / MB)} MB. After that it works offline and nothing leaves this computer.`;
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
  const total = Math.round(status.totalBytes / MB);
  const show = (done: number) => {
    progressBar.style.width = `${Math.min(100, (done / status.totalBytes) * 100)}%`;
    getStatus.textContent = `Downloading… ${Math.floor(done / MB)} of ${total} MB`;
  };
  show(0);
  void report();
  const onProgress = new Channel<{ done: number; total: number }>();
  onProgress.onmessage = (p) => show(p.done);
  log("info", `download of ${status.pair} started`);
  try {
    await invoke("translate_download", { pair: status.pair, onProgress });
    log("info", `download of ${status.pair} finished`);
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
    await invoke("translate_copy", { text: translation });
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
  void invoke("translate_set_pick", { script: request.script, pair: id }).catch((e) =>
    log("error", `remember pick: ${describe(e)}`),
  );
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
  closeMenu();
  sourceEl.textContent = req.text.length > 600 ? `${req.text.slice(0, 600)}…` : req.text;
  sourceEl.scrollTop = 0;
  shownAt = 0;
  log("info", `card opened (${req.script}, ${req.pair}, ${req.text.length} chars)`);
  await run();
}

void listen("translate://open", () => void open());
log("info", "page loaded");
void open();
