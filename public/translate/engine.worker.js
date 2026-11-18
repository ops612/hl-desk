/* HL Desk translation engine: Bergamot (Firefox Translations build) in a classic Web Worker,
 * single thread: no SharedArrayBuffer, no COOP/COEP, the fallback gemm.
 * This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0. If a copy of
 * the MPL was not distributed with this file, You can obtain one at http://mozilla.org/MPL/2.0/.
 * Adapted from Firefox toolkit/components/translations/content/translations-engine.worker.js.
 *
 * bergamot-translator.js: mozilla-central ab91df22f920 (v0.6.0+eea6e5a8), sha256 faff1ef6...797b47d.
 * bergamot-translator.wasm: Remote Settings translations-wasm v3.0, sha256 a3a89d9a...0182c2dc.
 * The page (src/translate/main.ts) fetches the .wasm and hands it over as an ArrayBuffer, so it is
 * compiled with WebAssembly.instantiate (no streaming, so the server's MIME type does not matter).
 *
 * One worker holds one language pair. Messages: {id, cmd: "init", wasm}, {id, cmd: "load", from,
 * to, files: {kind: {name, buffer}}}, {id, cmd: "translateMany", texts}; replies
 * {id, ok, result|error}.
 */
/* global loadBergamot */
importScripts("bergamot-translator.js");

const ALIGN = { model: 256, lex: 64, vocab: 64, srcvocab: 64, trgvocab: 64 };
let bergamot = null;
let service = null;
let model = null;
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

const heapMB = () => (bergamot ? +(bergamot.HEAP8.buffer.byteLength / 1048576).toFixed(1) : 0);

async function init(wasmBinary) {
  const t0 = performance.now();
  bergamot = await new Promise((resolve, reject) => {
    const m = loadBergamot({
      INITIAL_MEMORY: 41_943_040,
      print: (s) => logs.push(String(s)),
      printErr: (s) => logs.push("ERR " + s),
      onAbort: (what) => reject(new Error("engine aborted: " + what)),
      onRuntimeInitialized: async () => {
        await Promise.resolve();
        resolve(m);
      },
      wasmBinary,
    });
  });
  service = new bergamot.BlockingService({ cacheSize: 0 });
  return { ms: Math.round(performance.now() - t0), heapMB: heapMB() };
}

function load(from, to, files) {
  const t0 = performance.now();
  const mem = {};
  for (const [kind, f] of Object.entries(files)) {
    const a = new bergamot.AlignedMemory(f.buffer.byteLength, ALIGN[kind]);
    a.getByteArrayView().set(new Uint8Array(f.buffer));
    mem[kind] = a;
  }
  const vocabs = new bergamot.AlignedMemoryList();
  if (mem.vocab) vocabs.push_back(mem.vocab);
  else {
    vocabs.push_back(mem.srcvocab);
    vocabs.push_back(mem.trgvocab);
  }
  model = new bergamot.TranslationModel(from, to, config(files.model.name), mem.model, mem.lex ?? null, vocabs, null);
  return { ms: Math.round(performance.now() - t0), heapMB: heapMB() };
}

/** Translates several texts in one engine call (the editor's OCR lines, or one card text). */
function translateMany(texts) {
  if (!model) throw new Error("no language model loaded");
  const t0 = performance.now();
  const messages = new bergamot.VectorString();
  const options = new bergamot.VectorResponseOptions();
  for (const text of texts) {
    messages.push_back(text);
    options.push_back({ qualityScores: false, alignment: false, html: false });
  }
  const out = [];
  try {
    const res = service.translate(model, messages, options);
    for (let i = 0; i < texts.length; i++) out.push(res.get(i).getTranslatedText());
    res.delete();
  } finally {
    messages.delete();
    options.delete();
  }
  return { texts: out, ms: Math.round(performance.now() - t0), heapMB: heapMB() };
}

onmessage = async ({ data }) => {
  try {
    let result;
    if (data.cmd === "init") result = await init(data.wasm);
    else if (data.cmd === "load") result = load(data.from, data.to, data.files);
    else if (data.cmd === "translateMany") result = translateMany(data.texts);
    else throw new Error("unknown command " + data.cmd);
    postMessage({ id: data.id, ok: true, result, logs: logs.splice(0) });
  } catch (e) {
    postMessage({ id: data.id, ok: false, error: String((e && e.message) || e), logs: logs.splice(0) });
  }
};
