// Spike only: download the Bergamot engine and the newest desktop zh/en models
// from Mozilla Remote Settings, verify sha256, write models/manifest.json.
// Usage: node spike/translate/fetch-models.mjs
import { createHash } from "node:crypto";
import { mkdir, writeFile, readFile, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const RS = "https://firefox.settings.services.mozilla.com/v1/buckets/main/collections";
const CDN = "https://firefox-settings-attachments.cdn.mozilla.net/";
const PAIRS = [["zh-Hant", "en"], ["en", "zh-Hant"], ["zh-Hans", "en"]];

const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");
const vkey = (v) => v.split(/[.a-z]/).map((n) => Number(n) || 0);
const newer = (a, b) => { const x = vkey(a), y = vkey(b); for (let i = 0; i < 3; i++) if (x[i] !== y[i]) return x[i] > y[i]; return a > b; };
// Desktop, release channel: skip Android-only and nightly/beta-only records.
const desktopRelease = (f) => !f || f.includes("env.appinfo.OS != 'Android'");

async function fetchChecked(location, hash, out) {
  try { const b = await readFile(out); if (sha256(b) === hash) return b.length; } catch {}
  const res = await fetch(CDN + location);
  if (!res.ok) throw new Error(`${res.status} ${location}`);
  const buf = Buffer.from(await res.arrayBuffer());
  if (sha256(buf) !== hash) throw new Error(`sha256 mismatch ${location}`);
  await mkdir(dirname(out), { recursive: true });
  await writeFile(out, buf);
  return buf.length;
}

const manifest = { fetched: new Date().toISOString(), engine: null, pairs: {} };

// Engine
const wasm = (await (await fetch(`${RS}/translations-wasm/records`)).json()).data
  .filter((r) => r.name === "bergamot-translator")
  .reduce((a, r) => (!a || newer(r.version, a.version) ? r : a), null);
await fetchChecked(wasm.attachment.location, wasm.attachment.hash, join(here, "vendor", "bergamot-translator.wasm"));
manifest.engine = { version: wasm.version, release: wasm.release, license: wasm.license, size: wasm.attachment.size, sha256: wasm.attachment.hash, url: CDN + wasm.attachment.location };

// Models
const recs = (await (await fetch(`${RS}/translations-models/records`)).json()).data;
for (const [from, to] of PAIRS) {
  const rs = recs.filter((r) => r.fromLang === from && r.toLang === to && desktopRelease(r.filter_expression));
  const ver = rs.reduce((a, r) => (!a || newer(r.version, a) ? r.version : a), null);
  const key = `${from}-${to}`;
  manifest.pairs[key] = { version: ver, files: {} };
  for (const r of rs.filter((r) => r.version === ver)) {
    const out = join(here, "models", key, r.name);
    await fetchChecked(r.attachment.location, r.attachment.hash, out);
    manifest.pairs[key].files[r.fileType] = { name: r.name, size: r.attachment.size, sha256: r.attachment.hash, path: `models/${key}/${r.name}` };
    console.log(key, ver, r.fileType, r.name, (await stat(out)).size);
  }
}
await writeFile(join(here, "models", "manifest.json"), JSON.stringify(manifest, null, 2));
console.log("engine", manifest.engine.version, manifest.engine.release);
