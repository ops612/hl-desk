// Screenshot editor window (editor.html?id=<capture id>), opened by src-tauri/src/screenshot.rs.
// The edits are a list of operations drawn over the original image, so undo is "drop the last one".
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import "./editor.css";

type Tool = "number" | "pen" | "crop";
type Rect = { x: number; y: number; w: number; h: number };
type Op =
  | { kind: "marker"; id: number; x: number; y: number; color: string }
  | { kind: "move"; id: number; x: number; y: number }
  | { kind: "pen"; color: string; points: [number, number][] }
  | ({ kind: "crop" } & Rect);
type Marker = { id: number; n: number; x: number; y: number; color: string };

const isMac = navigator.userAgent.includes("Mac");
const id = Number(new URLSearchParams(location.search).get("id"));
const canvas = document.getElementById("canvas") as HTMLCanvasElement;
const ctx = canvas.getContext("2d")!;
const stage = document.getElementById("stage")!;
const statusEl = document.getElementById("status")!;
const applyCropBtn = document.getElementById("apply-crop") as HTMLButtonElement;
const undoBtn = document.getElementById("undo") as HTMLButtonElement;
const dragThumb = document.getElementById("drag-thumb") as HTMLImageElement;

const image = new Image();
let path = "";
let tool: Tool = "number";
let color = "#2F4CC8";
const ops: Op[] = [];
let version = 0; // bumps on every change; compared with savedVersion to skip needless saves
let savedVersion = 0;
let selection: Rect | null = null; // crop rectangle being dragged, not yet applied
let nextMarkerId = 1;

// Sizes scale with the capture's pixel density, so a marker looks the same on Retina and 1x screens.
const unit = Math.max(1, window.devicePixelRatio || 1);
const markerRadius = 12 * unit;

document.getElementById("keys")!.textContent =
  `N number · P pen · C crop · ${isMac ? "⌘Z" : "Ctrl+Z"} undo · Esc close`;
undoBtn.title = `Undo (${isMac ? "⌘Z" : "Ctrl+Z"})`;

// ---------- state from the operation list ----------

function replay(): { markers: Marker[]; pens: Extract<Op, { kind: "pen" }>[]; crop: Rect } {
  const markers = new Map<number, Marker>();
  const pens: Extract<Op, { kind: "pen" }>[] = [];
  let crop: Rect = { x: 0, y: 0, w: image.naturalWidth, h: image.naturalHeight };
  for (const op of ops) {
    if (op.kind === "marker") markers.set(op.id, { id: op.id, n: markers.size + 1, x: op.x, y: op.y, color: op.color });
    else if (op.kind === "move") Object.assign(markers.get(op.id)!, { x: op.x, y: op.y });
    else if (op.kind === "pen") pens.push(op);
    else crop = { x: op.x, y: op.y, w: op.w, h: op.h };
  }
  return { markers: [...markers.values()], pens, crop };
}

// ---------- drawing ----------

function render(withSelection = true): void {
  const { markers, pens, crop } = replay();
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
  ctx.lineWidth = 3 * unit;
  for (const pen of pens) {
    ctx.strokeStyle = pen.color;
    ctx.beginPath();
    pen.points.forEach(([x, y], i) => (i ? ctx.lineTo(x, y) : ctx.moveTo(x, y)));
    if (pen.points.length === 1) ctx.lineTo(pen.points[0][0] + 0.1, pen.points[0][1]);
    ctx.stroke();
  }

  ctx.textAlign = "center";
  ctx.textBaseline = "middle";
  ctx.font = `500 ${13 * unit}px "IBM Plex Sans", -apple-system, sans-serif`;
  for (const m of markers) {
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

  if (withSelection && selection) {
    const s = normalize(selection);
    ctx.fillStyle = "rgba(11, 18, 32, 0.45)";
    ctx.beginPath();
    ctx.rect(crop.x, crop.y, crop.w, crop.h);
    ctx.rect(s.x, s.y, s.w, s.h);
    ctx.fill("evenodd");
    ctx.setLineDash([6 * unit, 4 * unit]);
    ctx.lineWidth = unit;
    ctx.strokeStyle = "#fff";
    ctx.strokeRect(s.x, s.y, s.w, s.h);
    ctx.setLineDash([]);
  }
  undoBtn.disabled = ops.length === 0;
  applyCropBtn.hidden = !selection;
}

/** Shows the canvas at the capture's natural size (image pixels / screen scale), shrunk to fit. */
function fit(): void {
  const box = stage.getBoundingClientRect();
  const room = { w: box.width - 24, h: box.height - 24 };
  const scale = Math.min(room.w / canvas.width, room.h / canvas.height, 1 / unit);
  canvas.style.width = `${canvas.width * scale}px`;
  canvas.style.height = `${canvas.height * scale}px`;
}

function normalize(r: Rect): Rect {
  return {
    x: Math.round(Math.min(r.x, r.x + r.w)),
    y: Math.round(Math.min(r.y, r.y + r.h)),
    w: Math.round(Math.abs(r.w)),
    h: Math.round(Math.abs(r.h)),
  };
}

/** Pointer position in image pixels. */
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

// ---------- tools ----------

function setTool(next: Tool): void {
  if (next !== "crop") selection = null;
  tool = next;
  stage.dataset.tool = next;
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-tool]")) {
    b.setAttribute("aria-pressed", String(b.dataset.tool === next));
  }
  render();
}

function setColor(next: string): void {
  color = next;
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-color]")) {
    b.setAttribute("role", "radio");
    b.setAttribute("aria-checked", String(b.dataset.color === next));
  }
}

function markerAt(x: number, y: number): Marker | undefined {
  // Topmost first.
  return replay().markers.reverse().find((m) => Math.hypot(m.x - x, m.y - y) <= markerRadius + 2 * unit);
}

let drawing: Op | null = null; // pen stroke or marker move in progress
let pushed = false; // a marker move joins the list only once the pointer actually moves
canvas.addEventListener("pointerdown", (e) => {
  if (e.button !== 0) return;
  canvas.setPointerCapture(e.pointerId);
  const [x, y] = toImage(e);
  const hit = tool !== "crop" ? markerAt(x, y) : undefined;
  if (hit) {
    drawing = { kind: "move", id: hit.id, x, y };
    pushed = false;
  } else if (tool === "number") {
    ops.push({ kind: "marker", id: nextMarkerId++, x, y, color });
    changed();
  } else if (tool === "pen") {
    drawing = { kind: "pen", color, points: [[x, y]] };
    ops.push(drawing);
    pushed = true;
    render();
  } else {
    selection = { x, y, w: 0, h: 0 };
    render();
  }
});

canvas.addEventListener("pointermove", (e) => {
  if (!(e.buttons & 1)) return;
  const [x, y] = toImage(e);
  if (drawing?.kind === "move") {
    Object.assign(drawing, { x, y });
    if (!pushed) ops.push(drawing);
    pushed = true;
  } else if (drawing?.kind === "pen") drawing.points.push([x, y]);
  else if (tool === "crop" && selection) Object.assign(selection, { w: x - selection.x, h: y - selection.y });
  else return;
  render();
});

canvas.addEventListener("pointerup", () => {
  if (drawing) {
    drawing = null;
    if (pushed) changed();
  }
  if (selection) {
    const s = normalize(selection);
    if (s.w < 4 || s.h < 4) selection = null;
    render();
  }
});

function applyCrop(): void {
  if (!selection) return;
  ops.push({ kind: "crop", ...normalize(selection) });
  selection = null;
  changed();
}

function undo(): void {
  if (selection) {
    selection = null;
    render();
  } else if (ops.pop()) changed();
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

// "Drag me": saves the edits to the file, then starts a native file drag of it.
const dragArea = document.getElementById("drag")!;
let dragFrom: { x: number; y: number } | null = null;
dragArea.addEventListener("pointerdown", (e) => {
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
applyCropBtn.addEventListener("click", applyCrop);
document.getElementById("copy")!.addEventListener("click", () => run(copy));
document.getElementById("done")!.addEventListener("click", () => run(done));

document.addEventListener("keydown", (e) => {
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod && e.key.toLowerCase() === "z") {
    e.preventDefault();
    undo();
  } else if (mod && e.key.toLowerCase() === "c") {
    e.preventDefault();
    run(copy);
  } else if (e.key === "Enter" && selection) {
    applyCrop();
  } else if (e.key === "Escape") {
    // Esc first cancels a crop rectangle, then saves and closes the editor (same as Done).
    if (selection) {
      selection = null;
      render();
    } else run(done);
  } else if (!mod && !e.altKey) {
    const key = e.key.toLowerCase();
    if (key === "n") setTool("number");
    else if (key === "p") setTool("pen");
    else if (key === "c") setTool("crop");
  }
});
window.addEventListener("resize", fit);

// The red close button saves too, so markup is never lost. Tauri waits for this handler,
// then closes the window (needs core:window:allow-destroy in capabilities/editor.json).
void getCurrentWindow().onCloseRequested(async () => {
  await save().catch((e) => logError("save on close", e));
});

// ---------- start ----------

async function load(): Promise<void> {
  const shot = await invoke<{ path: string }>("screenshot_get", { id });
  path = shot.path;
  const bytes = await invoke<ArrayBuffer>("screenshot_read", { id });
  image.src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
  await image.decode();
  setColor(color);
  setTool("number");
  fit();
  dragThumb.src = thumbnail();
}

load().catch((e) => {
  logError("load", e);
  flash(String(e));
});
