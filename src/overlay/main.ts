// Area-capture overlay (Windows): one window per monitor (overlay.html?m=<monitor>), opened by
// src-tauri/src/screenshot.rs. Shows that monitor frozen; the user drags a rectangle.
// Esc or right-click cancels.
import "@fontsource/ibm-plex-sans/500.css";
import { invoke } from "@tauri-apps/api/core";
import "./overlay.css";

type Rect = { x: number; y: number; w: number; h: number };

const monitor = Number(new URLSearchParams(location.search).get("m"));
const shot = document.getElementById("shot") as HTMLImageElement;
const dim = document.getElementById("dim")!;
const sel = document.getElementById("sel")!;
const size = document.getElementById("size")!;
const hint = document.getElementById("hint")!;

let from: { x: number; y: number } | null = null;
let rect: Rect = { x: 0, y: 0, w: 0, h: 0 };
let finished = false;
let focusing = false;

function cancel(): void {
  if (finished) return;
  finished = true;
  void invoke("screenshot_overlay_cancel");
}

/** CSS pixels to the frozen image's physical pixels (the monitor's scale factor). */
function physical(r: Rect): Rect {
  const sx = shot.naturalWidth / window.innerWidth;
  const sy = shot.naturalHeight / window.innerHeight;
  return {
    x: Math.round(r.x * sx),
    y: Math.round(r.y * sy),
    w: Math.round(r.w * sx),
    h: Math.round(r.h * sy),
  };
}

function draw(): void {
  const dragging = from !== null || rect.w > 0;
  dim.hidden = dragging;
  sel.hidden = !dragging;
  hint.hidden = dragging;
  if (!dragging) return;
  Object.assign(sel.style, {
    left: `${rect.x}px`,
    top: `${rect.y}px`,
    width: `${rect.w}px`,
    height: `${rect.h}px`,
  });
  sel.classList.toggle("near-top", rect.y < 30);
  const p = physical(rect);
  size.textContent = `${p.w} × ${p.h}`;
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
  draw();
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
  draw();
});

document.addEventListener("pointermove", (e) => {
  // The keyboard (Esc) follows the pointer from one monitor's overlay to the next.
  if (!document.hasFocus() && !focusing) {
    focusing = true;
    invoke("screenshot_overlay_show").finally(() => (focusing = false));
  }
  track(e);
});

document.addEventListener("pointerup", (e) => {
  if (!from || e.button !== 0) return;
  track(e);
  from = null;
  const p = physical(rect);
  if (p.w < 4 || p.h < 4) {
    // A click, not a drag: start again.
    rect = { x: 0, y: 0, w: 0, h: 0 };
    draw();
    return;
  }
  finished = true;
  void invoke("screenshot_overlay_done", { index: monitor, x: p.x, y: p.y, width: p.w, height: p.h });
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

// Load the frozen screen, then ask Rust to show this window (it is created hidden, so no white flash).
invoke<ArrayBuffer>("screenshot_overlay_image", { index: monitor })
  .then((bytes) => {
    shot.src = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
    return shot.decode();
  })
  .then(() => invoke("screenshot_overlay_show"))
  .catch(cancel);
