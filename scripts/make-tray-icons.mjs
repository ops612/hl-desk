// Draws the tray / menu-bar icons and writes them to src-tauri/icons/tray/ (committed).
// Run: node scripts/make-tray-icons.mjs. Node only, no packages: shapes are rectangles and
// rounded rectangles, sampled 16 x 16 times per pixel, written as PNG (and one ICO).
//
// Windows: indigo #2F4CC8 rounded square with a white "HL", tray-<px>.png and tray.ico.
// macOS: template image (black + alpha): a rounded square with "HL" cut out, 18pt at 1x and 2x
// (tray-mac.png, tray-mac@2x.png); lib.rs sets icon_as_template(true).
// App icon source: src-tauri/icons/app-icon.png (1024 px, the same mark with the usual margin);
// then `npx tauri icon src-tauri/icons/app-icon.png -o <temp>` and copy the files listed in
// tauri.conf.json (bundle.icon) plus 64x64.png into src-tauri/icons/.
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { deflateSync } from "node:zlib";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "icons", "tray");
const INDIGO = [0x2f, 0x4c, 0xc8];
const SAMPLES_SMALL = 16;
/** Fewer samples for the 1024 px app icon (16 x 16 per pixel would take minutes). */
const SAMPLES_LARGE = 4;

// The design on a 16-unit grid: strokes 2 units wide, "HL" 12 x 8 units, centred.
const LETTERS = [
  [2, 4, 4, 12], // H left stem
  [6, 4, 8, 12], // H right stem
  [4, 7, 6, 9], // H bar
  [10, 4, 12, 12], // L stem
  [12, 10, 14, 12], // L foot
];
const RADIUS = 3.5;

const inRect = (x, y, [x0, y0, x1, y1]) => x >= x0 && x < x1 && y >= y0 && y < y1;
function inRounded(x, y, x0, y0, x1, y1, r) {
  if (x < x0 || x >= x1 || y < y0 || y >= y1) return false;
  const cx = Math.min(Math.max(x, x0 + r), x1 - r);
  const cy = Math.min(Math.max(y, y0 + r), y1 - r);
  return (x - cx) ** 2 + (y - cy) ** 2 <= r * r;
}

/** Renders size x size RGBA. paint(u, v) gets design units and returns [r, g, b, a] or null. */
function render(size, units, paint, SAMPLES = size > 128 ? SAMPLES_LARGE : SAMPLES_SMALL) {
  const px = new Uint8Array(size * size * 4);
  const scale = units / size;
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      let r = 0, g = 0, b = 0, a = 0;
      for (let sy = 0; sy < SAMPLES; sy++) {
        for (let sx = 0; sx < SAMPLES; sx++) {
          const c = paint((x + (sx + 0.5) / SAMPLES) * scale, (y + (sy + 0.5) / SAMPLES) * scale);
          if (!c) continue;
          // Premultiplied sums, so edges blend without dark fringes.
          r += c[0] * c[3]; g += c[1] * c[3]; b += c[2] * c[3]; a += c[3];
        }
      }
      const i = (y * size + x) * 4;
      if (a > 0) {
        px[i] = Math.round(r / a); px[i + 1] = Math.round(g / a); px[i + 2] = Math.round(b / a);
      }
      px[i + 3] = Math.round(a / (SAMPLES * SAMPLES));
    }
  }
  return px;
}

// Windows: indigo square, white letters.
const windowsPaint = (u, v) => {
  if (!inRounded(u, v, 0, 0, 16, 16, RADIUS)) return null;
  return LETTERS.some((r) => inRect(u, v, r)) ? [255, 255, 255, 255] : [...INDIGO, 255];
};
// macOS template: black square inset 1 unit on an 18-unit grid, letters cut out.
const macPaint = (u, v) => {
  if (!inRounded(u, v, 1, 1, 17, 17, RADIUS)) return null;
  return LETTERS.some((r) => inRect(u - 1, v - 1, r)) ? null : [0, 0, 0, 255];
};

// App icon: the same mark on a 20-unit grid, square inset 2 units (like macOS app icons).
const appPaint = (u, v) => {
  if (!inRounded(u, v, 2, 2, 18, 18, RADIUS)) return null;
  return LETTERS.some((r) => inRect(u - 2, v - 2, r)) ? [255, 255, 255, 255] : [...INDIGO, 255];
};

// ---- PNG and ICO writers ----
const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
  let c = n;
  for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  return c >>> 0;
});
function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}
function chunk(type, data) {
  const out = Buffer.alloc(12 + data.length);
  out.writeUInt32BE(data.length, 0);
  out.write(type, 4, "ascii");
  data.copy(out, 8);
  out.writeUInt32BE(crc32(out.subarray(4, 8 + data.length)), 8 + data.length);
  return out;
}
function png(size, rgba) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr.set([8, 6, 0, 0, 0], 8); // 8-bit RGBA
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    raw[y * (size * 4 + 1)] = 0; // filter: none
    Buffer.from(rgba.buffer, y * size * 4, size * 4).copy(raw, y * (size * 4 + 1) + 1);
  }
  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk("IHDR", ihdr),
    chunk("IDAT", deflateSync(raw, { level: 9 })),
    chunk("IEND", Buffer.alloc(0)),
  ]);
}
function ico(images) {
  const head = Buffer.alloc(6 + 16 * images.length);
  head.writeUInt16LE(1, 2);
  head.writeUInt16LE(images.length, 4);
  let offset = head.length;
  images.forEach(({ size, data }, i) => {
    const e = 6 + 16 * i;
    head[e] = size >= 256 ? 0 : size;
    head[e + 1] = size >= 256 ? 0 : size;
    head.writeUInt16LE(1, e + 4); // planes
    head.writeUInt16LE(32, e + 6); // bits per pixel
    head.writeUInt32LE(data.length, e + 8);
    head.writeUInt32LE(offset, e + 12);
    offset += data.length;
  });
  return Buffer.concat([head, ...images.map((i) => i.data)]);
}

mkdirSync(OUT, { recursive: true });
const windows = [16, 20, 24, 32, 40, 48, 64].map((size) => ({ size, data: png(size, render(size, 16, windowsPaint)) }));
for (const { size, data } of windows) writeFileSync(join(OUT, `tray-${size}.png`), data);
writeFileSync(join(OUT, "tray.ico"), ico(windows));
writeFileSync(join(OUT, "tray-mac.png"), png(18, render(18, 18, macPaint)));
writeFileSync(join(OUT, "tray-mac@2x.png"), png(36, render(36, 18, macPaint)));
writeFileSync(join(OUT, "..", "app-icon.png"), png(1024, render(1024, 20, appPaint)));
console.log(`Wrote ${windows.length} Windows sizes, tray.ico and the macOS template (18, 36 px) to ${OUT}`);
