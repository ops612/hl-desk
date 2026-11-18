/* Spike: Bergamot (Firefox build) in a plain classic Web Worker, single thread.
 * Adapted from Firefox toolkit/components/translations/content/translations-engine.worker.js (MPL-2.0).
 * vendor/bergamot-translator.js: mozilla-central ab91df22f920 (v0.6.0+eea6e5a8), sha256 faff1ef6...797b47d,
 *   https://hg-edge.mozilla.org/mozilla-central/raw-file/ab91df22f920/toolkit/components/translations/bergamot-translator/bergamot-translator.js
 * vendor/bergamot-translator.wasm (not committed): Remote Settings translations-wasm v3.0, fetched by fetch-models.mjs. */
/* global loadBergamot */
importScripts("vendor/bergamot-translator.js");

const ALIGN = { model: 256, lex: 64, vocab: 64, srcvocab: 64, trgvocab: 64 };
let bergamot = null;
let service = null;
const models = {};
const logs = [];

function config(modelName) {
  const c = {
    "beam-size": "1", normalize: "1.0", "word-penalty": "0",
    "max-length-break": "128", "mini-batch-words": "1024", workspace: "128",
    "max-length-factor": "2.0", "skip-cost": "true", "cpu-threads": "0",
    quiet: "true", "quiet-translation": "true",
    "gemm-precision": modelName.endsWith("intgemm8.bin") ? "int8shiftAll" : "int8shiftAlphaAll",
    alignment: "soft",
  };
  const ind = "            ";
  return "\n" + Object.entries(c).map(([k, v]) => `${ind}${k}: ${v}\n`).join("") + ind;
}

async function init() {
  const t0 = performance.now();
  const wasmBinary = await (await fetch("vendor/bergamot-translator.wasm")).arrayBuffer();
  const t1 = performance.now();
  bergamot = await new Promise((resolve, reject) => {
    const m = loadBergamot({
      INITIAL_MEMORY: 41_943_040,
      print: (s) => logs.push(String(s)),
      printErr: (s) => logs.push("ERR " + s),
      onAbort: () => reject(new Error("abort")),
      onRuntimeInitialized: async () => { await Promise.resolve(); resolve(m); },
      wasmBinary,
    });
  });
  service = new bergamot.BlockingService({ cacheSize: 0 });
  const t2 = performance.now();
  return { fetchMs: t1 - t0, initMs: t2 - t1, totalMs: t2 - t0, heapMB: heapMB(), logs: logs.splice(0) };
}

const heapMB = () => +(bergamot.HEAP8.buffer.byteLength / 1048576).toFixed(1);

async function load(pair, files) {
  const t0 = performance.now();
  const bufs = {};
  await Promise.all(Object.entries(files).map(async ([type, f]) => {
    bufs[type] = await (await fetch(f.path)).arrayBuffer();
  }));
  const t1 = performance.now();
  const mem = {};
  for (const [type, buf] of Object.entries(bufs)) {
    const a = new bergamot.AlignedMemory(buf.byteLength, ALIGN[type]);
    a.getByteArrayView().set(new Uint8Array(buf));
    mem[type] = a;
  }
  const vocabs = new bergamot.AlignedMemoryList();
  if (mem.vocab) vocabs.push_back(mem.vocab);
  else { vocabs.push_back(mem.srcvocab); vocabs.push_back(mem.trgvocab); }
  const [from, to] = pair.split(/-(?=en$|zh)/);
  models[pair] = new bergamot.TranslationModel(from, to, config(files.model.name), mem.model, mem.lex ?? null, vocabs, null);
  const t2 = performance.now();
  return { fetchMs: t1 - t0, buildMs: t2 - t1, totalMs: t2 - t0, heapMB: heapMB(), logs: logs.splice(0) };
}

function translate(pair, text) {
  const t0 = performance.now();
  const messages = new bergamot.VectorString();
  const options = new bergamot.VectorResponseOptions();
  messages.push_back(text);
  options.push_back({ qualityScores: false, alignment: true, html: false });
  let out;
  try {
    const res = service.translate(models[pair], messages, options);
    out = res.get(0).getTranslatedText();
    res.delete();
  } finally { messages.delete(); options.delete(); }
  return { text: out, ms: performance.now() - t0, heapMB: heapMB(), logs: logs.splice(0) };
}

onmessage = async ({ data }) => {
  try {
    let result;
    if (data.cmd === "env") result = { crossOriginIsolated: self.crossOriginIsolated, sab: typeof SharedArrayBuffer, mozIntGemm: typeof WebAssembly.mozIntGemm, ua: navigator.userAgent, cores: navigator.hardwareConcurrency };
    else if (data.cmd === "init") result = await init();
    else if (data.cmd === "load") result = await load(data.pair, data.files);
    else if (data.cmd === "translate") result = translate(data.pair, data.text);
    postMessage({ id: data.id, ok: true, result });
  } catch (e) {
    postMessage({ id: data.id, ok: false, error: String(e && e.stack || e), logs: logs.splice(0) });
  }
};
