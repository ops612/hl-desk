// Spike: tiny static server for spike/translate. Deliberately sends NO COOP/COEP headers.
// Usage: node spike/translate/serve.mjs [port]
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const port = Number(process.argv[2]) || 8765;
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".mjs": "text/javascript; charset=utf-8", ".json": "application/json", ".wasm": "application/wasm" };

createServer(async (req, res) => {
  const rel = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([\\/])+/, "") || "index.html";
  if (rel.startsWith("..")) { res.writeHead(403).end(); return; }
  try {
    const body = await readFile(join(root, rel));
    res.writeHead(200, { "Content-Type": TYPES[extname(rel)] || "application/octet-stream", "Cache-Control": "no-store" });
    res.end(body);
  } catch { res.writeHead(404).end("not found"); }
}).listen(port, "127.0.0.1", () => console.log(`http://127.0.0.1:${port}/`));
