// Screenshot editor window (editor.html?id=<capture id>), opened by src-tauri/src/screenshot.rs.
// The edits are a list of operations drawn over the original image, so undo is "move the last
// one to the redo list" and redo moves it back. Deleting a marker or stroke is an operation too,
// so it can be undone; the remaining markers renumber.
import { invoke } from "@tauri-apps/api/core";
import { pngUrl } from "../png-url";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import "@fontsource/ibm-plex-sans/600.css";
import "../styles.css";
import "../translate/translate.css";
import "./editor.css";
import { setupTranslatePanel } from "./translate-panel";
import { setupCopyText } from "./copy-text-button";
import { penCursor } from "./copy-text";

type Tool = "number" | "pen" | "crop";
type Rect = { x: number; y: number; w: number; h: number };
type Pen = { kind: "pen"; id: number; color: string; points: [number, number][] };
type Op =
  | { kind: "marker"; id: number; x: number; y: number; color: string }
  | { kind: "move"; id: number; x: number; y: number }
  | Pen
  | { kind: "delete"; id: number }
  | ({ kind: "crop" } & Rect);
type Marker = { id: number; n: number; x: number; y: number; color: string };
/** Which part of the crop rectangle is being dragged. */
type CropMode = "new" | "move" | "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw";

const isMac = navigator.userAgent.includes("Mac");
const id = Number(new URLSearchParams(location.search).get("id"));
const canvas = document.getElementById("canvas") as HTMLCanvasElement;
const ctx = canvas.getContext("2d")!;
const stage = document.getElementById("stage")!;
const statusEl = document.getElementById("status")!;
const applyCropBtn = document.getElementById("apply-crop") as HTMLButtonElement;
const undoBtn = document.getElementById("undo") as HTMLButtonElement;
const redoBtn = document.getElementById("redo") as HTMLButtonElement;
const dragThumb = document.getElementById("drag-thumb") as HTMLImageElement;
const pickTag = document.getElementById("pick-tag")!;

const image = new Image();
let path = "";
let tool: Tool = "number";
let color = "#2F4CC8";
const ops: Op[] = [];
const redoOps: Op[] = [];
let version = 0; // bumps on every change; compared with savedVersion to skip needless saves
let savedVersion = 0;
let selection: Rect | null = null; // crop rectangle, adjustable with its handles until applied
let picked: number | null = null; // id of the selected marker or pen stroke
let nextId = 1;

// Sizes scale with the capture's pixel density, so a marker looks the same on Retina and 1x screens.
const unit = Math.max(1, window.devicePixelRatio || 1);
const markerRadius = 12 * unit;
const penWidth = 3 * unit;

const mod = isMac ? "⌘" : "Ctrl+";
document.getElementById("keys")!.textContent =
  `N number · P pen · C crop · T copy text · click a mark, then Delete · ${mod}Z undo · ${isMac ? "⌘⇧Z" : "Ctrl+Shift+Z"} redo · Esc close`;
undoBtn.title = `Undo (${mod}Z)`;
redoBtn.title = `Redo (${isMac ? "⌘⇧Z" : "Ctrl+Shift+Z"})`;

// ---------- state from the operation list ----------

function replay(): { markers: Marker[]; pens: Pen[]; crop: Rect } {
  const markers = new Map<number, Marker>();
  const pens = new Map<number, Pen>();
  let crop: Rect = { x: 0, y: 0, w: image.naturalWidth, h: image.naturalHeight };
  for (const op of ops) {
    if (op.kind === "marker") markers.set(op.id, { id: op.id, n: 0, x: op.x, y: op.y, color: op.color });
    else if (op.kind === "move") {
      const m = markers.get(op.id);
      if (m) Object.assign(m, { x: op.x, y: op.y });
    } else if (op.kind === "pen") pens.set(op.id, op);
    else if (op.kind === "delete") {
      markers.delete(op.id);
      pens.delete(op.id);
    } else crop = { x: op.x, y: op.y, w: op.w, h: op.h };
  }
  // Numbers follow the order the markers were placed, so deleting one renumbers the rest.
  const list = [...markers.values()];
  list.forEach((m, i) => (m.n = i + 1));
  return { markers: list, pens: [...pens.values()], crop };
}

/** Adds an operation; a new edit clears the redo list. */
function push(op: Op): void {
  ops.push(op);
  redoOps.length = 0;
}

// ---------- drawing ----------

/** Image pixels per CSS pixel of the canvas on screen. */
function cssToImage(): number {
  const box = canvas.getBoundingClientRect();
  return box.width > 0 ? canvas.width / box.width : 1;
}

function drawPen(pen: Pen): void {
  ctx.beginPath();
  pen.points.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
  if (pen.points.length === 1) ctx.lineTo(pen.points[0][0] + 0.1, pen.points[0][1]);
  ctx.stroke();
}

function render(withUi = true): void {
  const { markers, pens, crop } = replay();
  if (picked !== null && !markers.some((m) => m.id === picked) && !pens.some((p) => p.id === picked)) {
    picked = null;
  }
  if (canvas.width !== crop.w || canvas.height !== crop.h) {
    canvas.width = crop.w;
    canvas.height = crop.h;
    fit();
  }
  ctx.setTransform(1, 0, 0, 1, -crop.x, -crop.y);
  ctx.clearRect(crop.x, crop.y, crop.w, crop.h);
  ctx.drawImage(image, 0, 0);

  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  for (const pen of pens) {
    if (withUi && pen.id === picked) {
      // Selected stroke: a light indigo halo under it.
      ctx.lineWidth = penWidth + 8 * unit;
      ctx.strokeStyle = "rgba(47, 76, 200, 0.35)";
      drawPen(pen);
    }
    ctx.lineWidth = penWidth;
    ctx.strokeStyle = pen.color;
    drawPen(pen);
  }

  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `500 ${13 * unit}px "IBM Plex Sans", -apple-system, sans-serif`;
  for (const m of markers) {
    if (withUi && m.id === picked) {
      // Selected marker: a ring around it.
      ctx.beginPath();
      ctx.arc(m.x, m.y, markerRadius + 5 * unit, 0, Math.PI * 2);
      ctx.lineWidth = 2.5 * unit;
      ctx.strokeStyle = "#2F4CC8";
      ctx.stroke();
    }
    ctx.fillStyle = m.color;
    ctx.beginPath();
    ctx.arc(m.x, m.y, markerRadius, 0, Math.PI * 2);
    ctx.fill();
    ctx.lineWidth = 2 * unit;
    ctx.strokeStyle = "#fff";
    ctx.stroke();
    ctx.fillStyle = "#fff";
    ctx.fillText(String(m.n), m.x, m.y + unit / 2);
  }

  if (withUi && selection) drawCropSelection(normalize(selection), crop);
  if (withUi) placePickTag(markers, pens, crop);
  undoBtn.disabled = ops.length === 0 && !selection;
  redoBtn.disabled = redoOps.length === 0;
  applyCropBtn.hidden = !selection || tool !== "crop";
}

function drawCropSelection(s: Rect, crop: Rect): void {
  ctx.fillStyle = "rgba(11, 18, 32, 0.45)";
  ctx.beginPath();
  ctx.rect(crop.x, crop.y, crop.w, crop.h);
  ctx.rect(s.x, s.y, s.w, s.h);
  ctx.fill("evenodd");
  const px = cssToImage();
  ctx.setLineDash([6 * px, 4 * px]);
  ctx.lineWidth = px;
  ctx.strokeStyle = "#fff";
  ctx.strokeRect(s.x, s.y, s.w, s.h);
  ctx.setLineDash([]);
  // Eight handles, 8 CSS pixels each whatever the zoom.
  const size = 8 * px;
  ctx.fillStyle = "#fff";
  ctx.strokeStyle = "#2F4CC8";
  ctx.lineWidth = 1.5 * px;
  for (const [hx, hy] of handlePoints(s)) {
    ctx.fillRect(hx - size / 2, hy - size / 2, size, size);
    ctx.strokeRect(hx - size / 2, hy - size / 2, size, size);
  }
}

function handlePoints(s: Rect): [number, number][] {
  const { x, y, w, h } = s;
  return [
    [x, y], [x + w / 2, y], [x + w, y], [x + w, y + h / 2],
    [x + w, y + h], [x + w / 2, y + h], [x, y + h], [x, y + h / 2],
  ];
}

/** Puts "Delete to remove" just above the selected marker or stroke. */
function placePickTag(markers: Marker[], pens: Pen[], crop: Rect): void {
  let at: [number, number] | null = null;
  const m = markers.find((m) => m.id === picked);
  const p = pens.find((p) => p.id === picked);
  if (m) at = [m.x, m.y - markerRadius - 6 * unit];
  else if (p) {
    const xs = p.points.map((q) => q[0]);
    const ys = p.points.map((q) => q[1]);
    at = [(Math.min(...xs) + Math.max(...xs)) / 2, Math.min(...ys) - 6 * unit];
  }
  pickTag.hidden = !at;
  if (!at) return;
  const box = canvas.getBoundingClientRect();
  const host = stage.getBoundingClientRect();
  const scale = box.width / canvas.width;
  const left = box.left - host.left + (at[0] - crop.x) * scale;
  const top = Math.max(24, box.top - host.top + (at[1] - crop.y) * scale - 4);
  pickTag.style.left = `${left}px`;
  pickTag.style.top = `${top}px`;
}

/** Shows the canvas at the capture's natural size (image pixels / screen scale), shrunk to fit. */
function fit(): void {
  const box = stage.getBoundingClientRect();
  const room = { w: box.width - 24, h: box.height - 24 };
  const scale = Math.min(room.w / canvas.width, room.h / canvas.height, 1 / unit);
  canvas.style.width = `${canvas.width * scale}px`;
  canvas.style.height = `${canvas.height * scale}px`;
  updatePenCursor();
}

/** Pen cursor: a dot the size of the stroke on screen in the chosen colour (editor.css fallback: crosshair). */
function updatePenCursor(): void {
  stage.style.setProperty("--pen-cursor", penCursor(color, penWidth / cssToImage()));
}

function normalize(r: Rect): Rect {
  return {
    x: Math.round(Math.min(r.x, r.x + r.w)),
    y: Math.round(Math.min(r.y, r.y + r.h)),
    w: Math.round(Math.abs(r.w)),
    h: Math.round(Math.abs(r.h)),
  };
}

/** Pointer position in image pixels, kept inside the current crop. */
function toImage(e: PointerEvent): [number, number] {
  const { crop } = replay();
  const box = canvas.getBoundingClientRect();
  const x = crop.x + ((e.clientX - box.left) * canvas.width) / box.width;
  const y = crop.y + ((e.clientY - box.top) * canvas.height) / box.height;
  return [Math.min(Math.max(x, crop.x), crop.x + crop.w), Math.min(Math.max(y, crop.y), crop.y + crop.h)];
}

function changed(): void {
  version++;
  render();
  updateThumb();
}

let thumbTimer = 0;
function updateThumb(): void {
  clearTimeout(thumbTimer);
  thumbTimer = window.setTimeout(() => (dragThumb.src = thumbnail()), 150);
}

/** A small PNG of the current edit, used as the preview and as the drag icon. */
function thumbnail(): string {
  render(false);
  const scale = Math.min(1, 128 / Math.max(canvas.width, canvas.height));
  const small = document.createElement("canvas");
  small.width = Math.max(1, Math.round(canvas.width * scale));
  small.height = Math.max(1, Math.round(canvas.height * scale));
  small.getContext("2d")!.drawImage(canvas, 0, 0, small.width, small.height);
  render();
  return small.toDataURL("image/png");
}

// ---------- hit tests ----------

function markerAt(x: number, y: number): Marker | undefined {
  // Topmost first.
  return replay().markers.reverse().find((m) => Math.hypot(m.x - x, m.y - y) <= markerRadius + 2 * unit);
}

function distanceToSegment(px: number, py: number, a: [number, number], b: [number, number]): number {
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  const len = dx * dx + dy * dy;
  const t = len ? Math.max(0, Math.min(1, ((px - a[0]) * dx + (py - a[1]) * dy) / len)) : 0;
  return Math.hypot(px - (a[0] + t * dx), py - (a[1] + t * dy));
}

function penAt(x: number, y: number): Pen | undefined {
  const reach = penWidth / 2 + 5 * cssToImage();
  return replay()
    .pens.reverse()
    .find((pen) =>
      pen.points.length === 1
        ? Math.hypot(pen.points[0][0] - x, pen.points[0][1] - y) <= reach
        : pen.points.some((p, i) => i > 0 && distanceToSegment(x, y, pen.points[i - 1], p) <= reach),
    );
}

/** Which crop handle (or the inside) is under the pointer. */
function cropModeAt(x: number, y: number): CropMode {
  if (!selection) return "new";
  const s = normalize(selection);
  const reach = 7 * cssToImage();
  const near = (a: number, b: number) => Math.abs(a - b) <= reach;
  const inX = x >= s.x - reach && x <= s.x + s.w + reach;
  const inY = y >= s.y - reach && y <= s.y + s.h + reach;
  const top = near(y, s.y) && inX;
  const bottom = near(y, s.y + s.h) && inX;
  const left = near(x, s.x) && inY;
  const right = near(x, s.x + s.w) && inY;
  if (top && left) return "nw";
  if (top && right) return "ne";
  if (bottom && left) return "sw";
  if (bottom && right) return "se";
  if (top) return "n";
  if (bottom) return "s";
  if (left) return "w";
  if (right) return "e";
  if (x > s.x && x < s.x + s.w && y > s.y && y < s.y + s.h) return "move";
  return "new";
}

const CURSORS: Record<CropMode, string> = {
  new: "", move: "move", n: "ns-resize", s: "ns-resize", e: "ew-resize", w: "ew-resize",
  ne: "nesw-resize", sw: "nesw-resize", nw: "nwse-resize", se: "nwse-resize",
};

// ---------- tools ----------

function setTool(next: Tool): void {
  if (next !== "crop") selection = null;
  tool = next;
  stage.dataset.tool = next;
  canvas.style.cursor = "";
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-tool]")) {
    b.setAttribute("aria-pressed", String(b.dataset.tool === next));
  }
  render();
}

function setColor(next: string): void {
  color = next;
  updatePenCursor();
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-color]")) {
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(b.dataset.color === next));
  }
}

let drawing: Op | null = null; // pen stroke or marker move in progress
let pushed = false; // a move (or a pen press on a stroke) joins the list only once the pointer moves
let tapOnPen: number | null = null; // pen tool pressed on a stroke: a tap without moving selects it
let cropDrag: { mode: CropMode; from: [number, number]; orig: Rect } | null = null;

canvas.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  canvas.setPointerCapture(e.pointerId);
  const [x, y] = toImage(e);

  if (tool === "crop") {
    const mode = cropModeAt(x, y);
    const orig = selection ? normalize(selection) : { x, y, w: 0, h: 0 };
    if (mode === "new") selection = { x, y, w: 0, h: 0 };
    cropDrag = { mode, from: [x, y], orig };
    render();
    return;
  }

  const marker = markerAt(x, y);
  if (marker) {
    // Select it; dragging moves it.
    picked = marker.id;
    drawing = { kind: "move", id: marker.id, x, y };
    pushed = false;
    render();
    return;
  }
  const pen = penAt(x, y);
  if (tool === "number") {
    if (pen) {
      picked = pen.id;
      render();
      return;
    }
    picked = null;
    push({ kind: "marker", id: nextId++, x, y, color });
    changed();
  } else {
    picked = null;
    drawing = { kind: "pen", id: nextId++, color, points: [[x, y]] };
    tapOnPen = pen ? pen.id : null;
    pushed = !pen; // on a stroke, wait: a tap selects it, a drag draws
    if (pushed) push(drawing);
    render();
  }
});

canvas.addEventListener("pointermove", (e) => {
  const [x, y] = toImage(e);
  if (!(e.buttons & 1)) {
    // Hover: show what a press would do.
    if (tool === "crop") canvas.style.cursor = CURSORS[cropModeAt(x, y)];
    else if (markerAt(x, y)) canvas.style.cursor = "move";
    else if (tool === "number" && penAt(x, y)) canvas.style.cursor = "pointer";
    else canvas.style.cursor = "";
    return;
  }
  if (cropDrag) {
    moveCrop(cropDrag, x, y);
  } else if (drawing?.kind === "move") {
    Object.assign(drawing, { x, y });
    if (!pushed) push(drawing);
    pushed = true;
  } else if (drawing?.kind === "pen") {
    drawing.points.push([x, y]);
    if (!pushed) push(drawing);
    pushed = true;
    tapOnPen = null;
  } else return;
  render();
});

function moveCrop(drag: { mode: CropMode; from: [number, number]; orig: Rect }, x: number, y: number): void {
  const { crop } = replay();
  const o = drag.orig;
  if (drag.mode === "new") {
    selection = { x: drag.from[0], y: drag.from[1], w: x - drag.from[0], h: y - drag.from[1] };
  } else if (drag.mode === "move") {
    const nx = Math.min(Math.max(o.x + x - drag.from[0], crop.x), crop.x + crop.w - o.w);
    const ny = Math.min(Math.max(o.y + y - drag.from[1], crop.y), crop.y + crop.h - o.h);
    selection = { x: nx, y: ny, w: o.w, h: o.h };
  } else {
    let left = o.x;
    let top = o.y;
    let right = o.x + o.w;
    let bottom = o.y + o.h;
    if (drag.mode.includes("n")) top = y;
    if (drag.mode.includes("s")) bottom = y;
    if (drag.mode.includes("w")) left = x;
    if (drag.mode.includes("e")) right = x;
    selection = { x: left, y: top, w: right - left, h: bottom - top };
  }
}

canvas.addEventListener("pointerup", () => {
  if (drawing) {
    if (drawing.kind === "pen" && !pushed && tapOnPen !== null) {
      // A tap on a stroke with the pen tool: select that stroke instead of drawing a dot.
      picked = tapOnPen;
      nextId--; // the dot was never added
    }
    drawing = null;
    tapOnPen = null;
    if (pushed) changed();
    else render();
  }
  if (cropDrag) {
    cropDrag = null;
    if (selection) {
      const s = normalize(selection);
      selection = s.w < 4 || s.h < 4 ? null : s;
    }
    render();
  }
});

function applyCrop(): void {
  if (!selection) return;
  push({ kind: "crop", ...normalize(selection) });
  selection = null;
  canvas.style.cursor = "";
  changed();
}

function deletePicked(): void {
  if (picked === null) return;
  push({ kind: "delete", id: picked });
  picked = null;
  changed();
}

function undo(): void {
  if (selection) {
    selection = null;
    render();
    return;
  }
  const op = ops.pop();
  if (op) {
    redoOps.push(op);
    changed();
  }
}

function redo(): void {
  const op = redoOps.pop();
  if (op) {
    ops.push(op);
    changed();
  }
}

// ---------- save, copy, drag, close ----------

// One save at a time: a second call waits for the running one, then saves only if more changed.
let saving: Promise<void> | null = null;
async function save(): Promise<void> {
  while (saving) await saving.catch(() => {});
  if (version === savedVersion) return;
  saving = writePng();
  try {
    await saving;
  } finally {
    saving = null;
  }
}

async function writePng(): Promise<void> {
  const at = version;
  render(false);
  const blob = await new Promise<Blob | null>((resolve) => canvas.toBlob(resolve, "image/png"));
  render();
  if (!blob) throw new Error("Could not make the PNG.");
  await invoke("screenshot_save", new Uint8Array(await blob.arrayBuffer()), {
    headers: { "x-shot-id": String(id) },
  });
  savedVersion = at;
}

async function copy(): Promise<void> {
  render(false);
  const { data, width, height } = ctx.getImageData(0, 0, canvas.width, canvas.height);
  render();
  await invoke("screenshot_copy", new Uint8Array(data.buffer), {
    headers: { "x-width": String(width), "x-height": String(height) },
  });
  flash("Copied");
}

async function done(): Promise<void> {
  await save();
  await invoke("screenshot_close");
}

let flashTimer = 0;
function flash(text: string): void {
  statusEl.textContent = text;
  clearTimeout(flashTimer);
  flashTimer = window.setTimeout(() => (statusEl.textContent = ""), 2500);
}

/** Writes a failure to the app log (hl-desk.log); the window has no console in a release build. */
function logError(step: string, e: unknown): void {
  void invoke("app_log", { level: "error", message: `editor ${id}: ${step}: ${String(e)}` }).catch(
    () => {},
  );
}

function run(action: () => Promise<void>): void {
  action().catch((e) => {
    logError("action", e);
    flash(String(e));
  });
}

// "Drag to share": saves the edits to the file, then starts a native file drag of it.
const dragArea = document.getElementById("drag")!;
let dragFrom: { x: number; y: number } | null = null;
dragArea.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  e.preventDefault();
  dragFrom = { x: e.clientX, y: e.clientY };
  run(save); // usually finished before the pointer has moved far enough
});
dragArea.addEventListener("pointermove", (e) => {
  if (!dragFrom || !(e.buttons & 1)) return;
  if (Math.hypot(e.clientX - dragFrom.x, e.clientY - dragFrom.y) < 4) return;
  dragFrom = null;
  run(async () => {
    await save();
    await startDrag({ item: [path], icon: thumbnail() });
  });
});
dragArea.addEventListener("pointerup", () => (dragFrom = null));

for (const b of document.querySelectorAll<HTMLButtonElement>("[data-tool]")) {
  b.addEventListener("click", () => setTool(b.dataset.tool as Tool));
}
for (const b of document.querySelectorAll<HTMLButtonElement>("[data-color]")) {
  b.addEventListener("click", () => setColor(b.dataset.color!));
}
undoBtn.addEventListener("click", undo);
redoBtn.addEventListener("click", redo);
applyCropBtn.addEventListener("click", applyCrop);
document.getElementById("copy")!.addEventListener("click", () => run(copy));
document.getElementById("done")!.addEventListener("click", () => run(done));

document.addEventListener("keydown", (e) => {
  const cmd = isMac ? e.metaKey : e.ctrlKey;
  const key = e.key.toLowerCase();
  if (cmd && key === "z" && e.shiftKey) {
    e.preventDefault();
    redo();
  } else if (cmd && key === "y" && !isMac) {
    e.preventDefault();
    redo();
  } else if (cmd && key === "z") {
    e.preventDefault();
    undo();
  } else if (cmd && key === "c") {
    e.preventDefault();
    run(copy);
  } else if (e.key === "Enter" && selection) {
    applyCrop();
  } else if ((e.key === "Delete" || e.key === "Backspace") && picked !== null) {
    e.preventDefault();
    deletePicked();
  } else if (e.key === "Escape") {
    // Esc first cancels a crop rectangle, then clears the selection, then saves and closes
    // the editor (same as Done).
    if (selection) {
      selection = null;
      render();
    } else if (picked !== null) {
      picked = null;
      render();
    } else run(done);
  } else if (!cmd && !e.altKey) {
    if (key === "n") setTool("number");
    else if (key === "p") setTool("pen");
    else if (key === "c") setTool("crop");
    else if (key === "t") copyText();
  }
});
window.addEventListener("resize", () => {
  fit();
  render();
});
// Translate button and panel: the stage gets narrower or wider, so the canvas refits.
setupTranslatePanel(id, () => {
  fit();
  render();
});
const copyText = setupCopyText(id);
// The toolbar can wrap to a second row (a narrow window, or the Copy text label growing for a
// moment), which makes the stage shorter: refit the canvas then too.
new ResizeObserver(() => {
  if (!image.naturalWidth) return; // not loaded yet: load() fits
  fit();
  render();
}).observe(stage);

// The red close button saves too, so markup is never lost. Tauri waits for this handler,
// then closes the window (needs core:window:allow-destroy in capabilities/editor.json).
void getCurrentWindow().onCloseRequested(async () => {
  await save().catch((e) => logError("save on close", e));
});

// ---------- start ----------

async function load(): Promise<void> {
  const shot = await invoke<{ path: string }>("screenshot_get", { id });
  path = shot.path;
  const bytes = await invoke<unknown>("screenshot_read", { id });
  image.src = pngUrl(bytes);
  await image.decode();
  setColor(color);
  setTool("number");
  fit();
  render();
  dragThumb.src = thumbnail();
}

load().catch((e) => {
  logError("load", e);
  flash(String(e));
});
