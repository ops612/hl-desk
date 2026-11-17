// Writes latest.json, the update manifest HL Desk reads from
// https://github.com/ops612/hl-desk-releases/releases/latest/download/latest.json
// (Tauri's static format; steps in docs/release.md). Node only, no dependencies.
//
//   node scripts/make-latest-json.mjs --version 0.1.4 --notes "What changed" \
//     --mac <HL-Desk-mac.app.tar.gz> --win <HL-Desk-setup.exe> [--out latest.json]
//
// Each file's signature is read from "<file>.sig" (made by the CI build) and checked against
// the public key in src-tauri/tauri.conf.json, so a manifest the app would reject is never written.
// The URLs use the fixed file names of the release (HL-Desk-mac.app.tar.gz, HL-Desk-setup.exe).
import { createHash, createPublicKey, verify } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const RELEASES = "https://github.com/ops612/hl-desk-releases/releases/download";
const MAC_NAME = "HL-Desk-mac.app.tar.gz";
const WIN_NAME = "HL-Desk-setup.exe";

function fail(message) {
  console.error(`make-latest-json: ${message}`);
  process.exit(1);
}

function args() {
  const out = {};
  const list = process.argv.slice(2);
  for (let i = 0; i < list.length; i += 2) {
    const key = list[i];
    if (!key.startsWith("--") || list[i + 1] === undefined) fail(`bad argument near "${key}"`);
    out[key.slice(2)] = list[i + 1];
  }
  for (const k of ["version", "notes", "mac", "win"]) if (!out[k]) fail(`--${k} is required`);
  if (!/^\d+\.\d+\.\d+$/.test(out.version)) fail(`--version must look like 0.1.4, got "${out.version}"`);
  return out;
}

/** A minisign key or signature line: base64 of [2-byte algorithm][8-byte key id][key or signature]. */
function decodeLine(line, what) {
  const bytes = Buffer.from(line.trim(), "base64");
  if (bytes.length < 10) fail(`${what}: not a minisign line`);
  return { alg: bytes.subarray(0, 2).toString("latin1"), keyId: bytes.subarray(2, 10), rest: bytes.subarray(10) };
}

/** The public key from tauri.conf.json (base64 of the minisign .pub file). */
function publicKey() {
  const conf = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "tauri.conf.json");
  const pub = JSON.parse(readFileSync(conf, "utf8")).plugins?.updater?.pubkey;
  if (!pub) fail(`no plugins.updater.pubkey in ${conf}`);
  const lines = Buffer.from(pub, "base64").toString("utf8").split(/\r?\n/).filter(Boolean);
  const key = decodeLine(lines[1] ?? "", "public key");
  if (key.alg !== "Ed" || key.rest.length !== 32) fail("public key: not an Ed25519 minisign key");
  const jwk = { kty: "OKP", crv: "Ed25519", x: key.rest.toString("base64url") };
  return { keyId: key.keyId, key: createPublicKey({ key: jwk, format: "jwk" }) };
}

/** Reads "<file>.sig", checks it signs <file> with our key, and returns it (as the app expects it). */
function signature(file, pub, version) {
  if (!existsSync(file)) fail(`file not found: ${file}`);
  const sigFile = `${file}.sig`;
  if (!existsSync(sigFile)) fail(`signature not found: ${sigFile}`);
  const sig = readFileSync(sigFile, "utf8").trim();
  if (!sig) fail(`signature is empty: ${sigFile}`);
  const lines = Buffer.from(sig, "base64").toString("utf8").split(/\r?\n/);
  const s = decodeLine(lines[1] ?? "", sigFile);
  const trusted = (lines[2] ?? "").replace(/^trusted comment: /, "");
  const global = Buffer.from((lines[3] ?? "").trim(), "base64");
  if (!s.keyId.equals(pub.keyId)) fail(`${sigFile}: signed with another key than tauri.conf.json's`);
  // "ED": signature over the BLAKE2b-512 hash of the file (what Tauri makes); "Ed": over the file.
  const data = readFileSync(file);
  const message = s.alg === "ED" ? createHash("blake2b512").update(data).digest() : data;
  if (!verify(null, message, pub.key, s.rest)) fail(`${sigFile}: does not match ${file}`);
  if (!verify(null, Buffer.concat([s.rest, Buffer.from(trusted, "utf8")]), pub.key, global)) {
    fail(`${sigFile}: trusted comment signature is invalid`);
  }
  // The signed file name may carry the version (Windows: "HL Desk_0.1.4_x64-setup.exe").
  const signedVersion = /_(\d+\.\d+\.\d+)_/.exec(trusted)?.[1];
  if (signedVersion && signedVersion !== version) fail(`${sigFile}: built as ${signedVersion}, not ${version}`);
  return sig;
}

const a = args();
const pub = publicKey();
const mac = { signature: signature(a.mac, pub, a.version), url: `${RELEASES}/v${a.version}/${MAC_NAME}` };
const win = { signature: signature(a.win, pub, a.version), url: `${RELEASES}/v${a.version}/${WIN_NAME}` };
const manifest = {
  version: a.version,
  notes: a.notes,
  pub_date: new Date().toISOString().replace(/\.\d{3}Z$/, "Z"),
  platforms: {
    // One universal .app serves both Mac architectures.
    "darwin-aarch64": mac,
    "darwin-x86_64": mac,
    "windows-x86_64": win,
  },
};
const out = a.out ?? "latest.json";
writeFileSync(out, `${JSON.stringify(manifest, null, 2)}\n`, "utf8");
console.log(`make-latest-json: wrote ${out} (${a.version}, signatures checked)`);
