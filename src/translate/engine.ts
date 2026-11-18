// Shared translation engine for the translate card (src/translate/main.ts) and the screenshot
// editor's Translate panel. The engine (Bergamot, public/translate/) runs in a Web Worker that
// holds one language pair; it is stopped after 2 minutes idle to give memory back (about 330 MB
// per pair once used) and when another pair is needed. Model files come from Rust
// (translate_model_file) and are downloaded on first use (translate_download).
import { Channel, invoke } from "@tauri-apps/api/core";

export type PairId = "zh-Hant-en" | "zh-Hans-en" | "en-zh-Hant";
export type Script = "han" | "latin";
export interface PairStatus {
  pair: PairId;
  installed: boolean;
  totalBytes: number;
  files: { kind: string; name: string }[];
}

export const PAIRS: { id: PairId; label: string; from: string; to: string }[] = [
  { id: "zh-Hant-en", label: "Chinese (Traditional) → English", from: "zh-Hant", to: "en" },
  { id: "zh-Hans-en", label: "Chinese (Simplified) → English", from: "zh-Hans", to: "en" },
  { id: "en-zh-Hant", label: "English → Chinese (Traditional)", from: "en", to: "zh-Hant" },
];
export const pairInfo = (id: PairId) => PAIRS.find((p) => p.id === id)!;
export const MB = 1_000_000;
const IDLE_MS = 2 * 60 * 1000;

export function log(level: "info" | "warn" | "error", message: string): void {
  void invoke("app_log", { level, message: `translate: ${message}` }).catch(() => {});
}
export const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));

// ---------- worker ----------

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

/** Restarts the 2-minute idle timer (call after each use and when a view closes). */
export function touchIdle(): void {
  clearTimeout(idleTimer);
  idleTimer = window.setTimeout(() => stopEngine("2 minutes idle"), IDLE_MS);
}

/** Sends a message to worker `w`, only while it is still the current one (a start-up for a pair
 * that was replaced meanwhile must not reach the new worker). */
function call<T>(w: Worker, msg: Record<string, unknown>, transfer: Transferable[] = []): Promise<T> {
  return new Promise((res, rej) => {
    if (!worker || worker !== w) return rej(new Error("The translation engine was restarted."));
    const id = ++callSeq;
    calls.set(id, { res, rej });
    w.postMessage({ ...msg, id }, transfer);
  });
}

/** Throws when worker `w` was replaced or stopped (abandons a stale start-up). */
function checkCurrent(w: Worker): void {
  if (worker !== w) throw new Error("The translation engine was restarted.");
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
    checkCurrent(w);
    // The engine uses WebAssembly SIMD (Safari / WebKit 16.4 or newer); say so instead of a
    // compile error on an older Mac.
    if (!WebAssembly.validate(wasm)) {
      throw new Error("This computer's web engine is too old for translation (on a Mac: Safari 16.4 or newer).");
    }
    const init = await call<{ ms: number; heapMB: number }>(w, { cmd: "init", wasm }, [wasm]);
    log("info", `engine started in ${init.ms} ms (${init.heapMB} MB)`);
    const files: Record<string, { name: string; buffer: ArrayBuffer }> = {};
    for (const f of status.files) {
      const buffer = toBuffer(await invoke("translate_model_file", { pair: status.pair, kind: f.kind }));
      checkCurrent(w);
      files[f.kind] = { name: f.name, buffer };
    }
    const buffers = Object.values(files).map((f) => f.buffer);
    const load = await call<{ ms: number; heapMB: number }>(w, { cmd: "load", from: info.from, to: info.to, files }, buffers);
    log("info", `${status.pair} loaded in ${Math.round(performance.now() - t0)} ms (build ${load.ms} ms, ${load.heapMB} MB)`);
  })();
  workerReady.catch((e) => {
    log("error", `engine start failed: ${describe(e)}`);
    if (worker === w) stopEngine("start failed");
  });
  return workerReady;
}

// ---------- text ----------

const HAN = "\\u3400-\\u4dbf\\u4e00-\\u9fff\\uf900-\\ufaff";
const ZH_PUNCT = new RegExp(`([${HAN}])\\s*([,;:?!])\\s*(?=[${HAN}])`, "g");
const FULL_WIDTH: Record<string, string> = { ",": "，", ";": "；", ":": "：", "?": "？", "!": "！" };

/** Chinese output: ASCII , ; : ? ! between two Han characters become full-width. */
export function fixZhPunctuation(text: string): string {
  return text.replace(ZH_PUNCT, (_m, han: string, p: string) => han + FULL_WIDTH[p]);
}

// ---------- public API ----------

export function pairStatus(pair: PairId): Promise<PairStatus> {
  return invoke<PairStatus>("translate_status", { pair });
}

/** Translates each text with an installed pair (one engine call for all of them). */
export async function translateTexts(status: PairStatus, texts: string[]): Promise<string[]> {
  clearTimeout(idleTimer);
  try {
    await ensureEngine(status);
    if (!worker || workerPair !== status.pair) throw new Error("The translation engine was restarted.");
    const out = await call<{ texts: string[]; ms: number; heapMB: number }>(worker, { cmd: "translateMany", texts });
    log("info", `translated ${texts.length} text(s), ${texts.join("").length} chars, with ${status.pair} in ${out.ms} ms (${out.heapMB} MB)`);
    const zh = pairInfo(status.pair).to.startsWith("zh");
    return out.texts.map((t) => (zh ? fixZhPunctuation(t) : t));
  } finally {
    touchIdle();
  }
}

/** Downloads the pair's model files; `onProgress(bytes done)`. */
export async function downloadPair(status: PairStatus, onProgress: (done: number) => void): Promise<void> {
  const channel = new Channel<{ done: number; total: number }>();
  channel.onmessage = (p) => onProgress(p.done);
  log("info", `download of ${status.pair} started`);
  await invoke("translate_download", { pair: status.pair, onProgress: channel });
  log("info", `download of ${status.pair} finished`);
}

/** Auto-detect, as for the card: the pair and source script for a text (None: no letters). */
export function detectPair(text: string): Promise<{ pair: PairId; script: Script } | null> {
  return invoke("translate_detect", { text });
}

/** Remembers a manual pick for text in `script`. */
export function rememberPick(script: Script, pair: PairId): void {
  void invoke("translate_set_pick", { script, pair }).catch((e) => log("error", `remember pick: ${describe(e)}`));
}

/** Copies a translation to the clipboard (the double-copy detector ignores it). */
export function copyTranslation(text: string): Promise<void> {
  return invoke("translate_copy", { text });
}

/** "Get <pair>" texts for the first-use download block. */
export function downloadTexts(status: PairStatus): { title: string; text: string; progress: (done: number) => string } {
  const total = Math.round(status.totalBytes / MB);
  return {
    title: `Get ${pairInfo(status.pair).label}`,
    text: `One download, about ${total} MB. After that it works offline and nothing leaves this computer.`,
    progress: (done) => `Downloading… ${Math.floor(done / MB)} of ${total} MB`,
  };
}
