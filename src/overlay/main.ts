// Area-capture overlay (Windows): one transparent window per monitor
// (overlay.html?m=<monitor>&w=<px width>&h=<px height>), opened by src-tauri/src/screenshot.rs.
// It shows itself at once with a dim layer, a crosshair and a large hint over the live screen,
// then draws that monitor's frozen pixels when they arrive. The user drags a rectangle; a plain
// click does nothing. Esc or right-click cancels (the capture key pressed again also cancels,
// in Rust). Crosshair lines and a magnifier with the pixel position / selection size follow
// the pointer.
import "@fontsource/ibm-plex-sans/500.css";
import { invoke } from "@tauri-apps/api/core";
import "./overlay.css";

type Rect = { x: number; y: number; w: number; h: number };

const params = new URLSearchParams(location.search);
const monitor = Number(params.get("m"));
/** The frozen image's size in physical pixels (the monitor's size). */
const imgW = Number(params.get("w")) || Math.round(window.innerWidth * devicePixelRatio);
const imgH = Number(params.get("h")) || Math.round(window.innerHeight * devicePixelRatio);
const shot = document.getElementById("shot") as HTMLCanvasElement;
const dim = document.getElementById("dim")!;
const sel = document.getElementById("sel")!;
const hint = document.getElementById("hint")!;
const hline = document.getElementById("hline")!;
const vline = document.getElementById("vline")!;
const loupe = document.getElementById("loupe")!;
const loupeCanvas = document.getElementById("loupe-canvas") as HTMLCanvasElement;
const loupeText = document.getElementById("loupe-text")!;
const loupeCtx = loupeCanvas.getContext("2d")!;

/** Magnifier: LOUPE_PX x LOUPE_PX image pixels shown in a LOUPE_CSS-point square. */
const LOUPE_PX = 15;
const LOUPE_CSS = 120;
loupeCanvas.width = LOUPE_CSS * devicePixelRatio;
loupeCanvas.height = LOUPE_CSS * devicePixelRatio;

let from: { x: number; y: number } | null = null;
let rect: Rect = { x: 0, y: 0, w: 0, h: 0 };
let pointer: { x: number; y: number } | null = null;
let imageReady = false;
let finished = false;
let focusing = false;

function log(level: "info" | "error", message: string): void {
  void invoke("app_log", { level, message: `overlay ${monitor}: ${message}` }).catch(() => {});
}

/** Esc or right-click. */
function cancel(): void {
  if (finished) return;
  finished = true;
  void invoke("screenshot_overlay_cancel");
}

/** Something failed: write it to the app log, then end the capture with the reason, which the
 * main window shows. Never a silent cancel. */
function fail(step: string, e: unknown): void {
  const detail = `${step}: ${e instanceof Error ? e.message : String(e)}`;
  const reason = `The capture overlay failed (${detail}).`;
  void invoke("app_log", { level: "error", message: `overlay ${monitor}: ${detail}` })
    .catch(() => {})
    .finally(() => {
      if (finished) return;
      finished = true;
      void invoke("screenshot_overlay_cancel", { reason });
    });
}

window.addEventListener("error", (e) => fail("script error", e.error ?? e.message));
window.addEventListener("unhandledrejection", (e) => fail("unhandled promise", e.reason));

/** CSS pixels to the frozen image's physical pixels (the monitor's scale factor). */
const scaleX = () => imgW / window.innerWidth;
const scaleY = () => imgH / window.innerHeight;
function physical(r: Rect): Rect {
  return {
    x: Math.round(r.x * scaleX()),
    y: Math.round(r.y * scaleY()),
    w: Math.round(r.w * scaleX()),
    h: Math.round(r.h * scaleY()),
  };
}

const dragging = () => from !== null || rect.w > 0;

function drawSelection(): void {
  const active = dragging();
  dim.hidden = active;
  sel.hidden = !active;
  hint.hidden = active;
  if (!active) return;
  Object.assign(sel.style, {
    left: `${rect.x}px`,
    top: `${rect.y}px`,
    width: `${rect.w}px`,
    height: `${rect.h}px`,
  });
}

/** Crosshair lines, magnifier and hint fading, once per frame. */
let frame = 0;
function drawPointer(): void {
  frame = 0;
  const p = pointer;
  hline.hidden = vline.hidden = !p;
  loupe.hidden = !p || !imageReady;
  if (!p) return;
  hline.style.top = `${p.y}px`;
  vline.style.left = `${p.x}px`;

  // The hint fades while the pointer is near it, so it never hides what is under it.
  const box = hint.getBoundingClientRect();
  const near =
    p.x > box.left - 60 && p.x < box.right + 60 && p.y > box.top - 60 && p.y < box.bottom + 60;
  hint.classList.toggle("faded", near);

  if (!imageReady) return;
  const px = Math.min(imgW - 1, Math.max(0, Math.floor(p.x * scaleX())));
  const py = Math.min(imgH - 1, Math.max(0, Math.floor(p.y * scaleY())));
  const size = loupeCanvas.width;
  const cell = size / LOUPE_PX;
  const half = Math.floor(LOUPE_PX / 2);
  loupeCtx.imageSmoothingEnabled = false;
  loupeCtx.fillStyle = "#0b1220";
  loupeCtx.fillRect(0, 0, size, size);
  loupeCtx.drawImage(shot, px - half, py - half, LOUPE_PX, LOUPE_PX, 0, 0, size, size);
  // The pixel under the pointer.
  loupeCtx.lineWidth = Math.max(1, devicePixelRatio);
  loupeCtx.strokeStyle = "#0b1220";
  loupeCtx.strokeRect(half * cell - 1, half * cell - 1, cell + 2, cell + 2);
  loupeCtx.strokeStyle = "#fff";
  loupeCtx.strokeRect(half * cell, half * cell, cell, cell);

  const r = physical(rect);
  loupeText.textContent = dragging() && r.w > 0 ? `${r.w} × ${r.h}` : `${px}, ${py}`;

  // Bottom-right of the pointer; flipped near the right and bottom edges.
  const gap = 24;
  const w = LOUPE_CSS;
  const h = LOUPE_CSS + 28;
  const left = p.x + gap + w > window.innerWidth ? p.x - gap - w : p.x + gap;
  const top = p.y + gap + h > window.innerHeight ? p.y - gap - h : p.y + gap;
  loupe.style.left = `${left}px`;
  loupe.style.top = `${top}px`;
}

function schedulePointer(): void {
  if (!frame) frame = requestAnimationFrame(drawPointer);
}

function track(e: PointerEvent): void {
  if (!from) return;
  const x = Math.max(0, Math.min(e.clientX, window.innerWidth));
  const y = Math.max(0, Math.min(e.clientY, window.innerHeight));
  rect = {
    x: Math.min(from.x, x),
    y: Math.min(from.y, y),
    w: Math.abs(x - from.x),
    h: Math.abs(y - from.y),
  };
  drawSelection();
}

document.addEventListener("pointerdown", (e) => {
  if (finished) return;
  if (e.button === 2) {
    cancel();
    return;
  }
  if (e.button !== 0) return;
  document.documentElement.setPointerCapture(e.pointerId);
  from = { x: e.clientX, y: e.clientY };
  rect = { x: e.clientX, y: e.clientY, w: 0, h: 0 };
  drawSelection();
});

document.addEventListener("pointermove", (e) => {
  // The keyboard (Esc) follows the pointer from one monitor's overlay to the next.
  if (!document.hasFocus() && !focusing) {
    focusing = true;
    invoke("screenshot_overlay_show")
      .catch((e) => fail("focusing the overlay", e))
      .finally(() => (focusing = false));
  }
  pointer = { x: e.clientX, y: e.clientY };
  track(e);
  schedulePointer();
});

document.documentElement.addEventListener("pointerleave", () => {
  if (from) return; // pointer capture: the drag continues
  pointer = null;
  schedulePointer();
});

document.addEventListener("pointerup", (e) => {
  if (!from || e.button !== 0) return;
  track(e);
  from = null;
  const p = physical(rect);
  if (p.w < 4 || p.h < 4) {
    // A plain click, not a drag: nothing happens; the user can drag again.
    rect = { x: 0, y: 0, w: 0, h: 0 };
    drawSelection();
    schedulePointer();
    return;
  }
  finished = true;
  invoke("screenshot_overlay_done", { index: monitor, x: p.x, y: p.y, width: p.w, height: p.h }).catch(
    (e) => {
      finished = false; // let fail() end the capture with the reason
      fail("sending the area", e);
    },
  );
});

document.addEventListener("contextmenu", (e) => {
  e.preventDefault();
  cancel();
});

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") {
    e.preventDefault();
    cancel();
  }
});

/** Raw RGBA from Rust: an ArrayBuffer normally, a number array on Tauri's fallback IPC path. */
function rgba(data: unknown): Uint8ClampedArray<ArrayBuffer> {
  if (data instanceof ArrayBuffer) return new Uint8ClampedArray(data);
  if (ArrayBuffer.isView(data))
    return new Uint8ClampedArray(
      data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer,
    );
  if (Array.isArray(data)) return Uint8ClampedArray.from(data as number[]);
  throw new Error(`unexpected image data (${Object.prototype.toString.call(data)})`);
}

// Show the window at once (dim layer, crosshair cursor and hint over the live screen; the window
// is created hidden, so there is no white flash), then draw the frozen pixels when they arrive.
// No requestAnimationFrame or image decode before showing: neither is guaranteed to run while
// the page is hidden.
const opened = performance.now();
shot.width = imgW;
shot.height = imgH;
invoke("screenshot_overlay_show").catch((e) => fail("showing the overlay", e));
invoke<unknown>("screenshot_overlay_image", { index: monitor })
  .then((data) => {
    const pixels = rgba(data);
    if (pixels.length !== imgW * imgH * 4) {
      throw new Error(`the frozen screen has ${pixels.length} bytes, expected ${imgW}x${imgH}x4`);
    }
    shot.getContext("2d")!.putImageData(new ImageData(pixels, imgW, imgH), 0, 0);
    imageReady = true;
    schedulePointer();
    log("info", `frozen screen drawn ${Math.round(performance.now() - opened)} ms after the page started`);
  })
  .catch((e) => fail("loading the frozen screen", e));
