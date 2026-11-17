// Screenshot feature. Called once, the first time the Screenshot tab is shown.
// Rust side: src-tauri/src/screenshot.rs. Editor window: editor.html + src/editor/.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import "./screenshot.css";

type Shot = { id: number; path: string; createdAt: number; updatedAt: number };

const KEEP_MS = 24 * 60 * 60 * 1000;
const isMac = navigator.userAgent.includes("Mac");
const shortcut = isMac ? "⌘⇧2" : "Ctrl+Shift+2";

// Thumbnail object URLs, keyed by "id:updatedAt" so an edited capture reloads.
const urls = new Map<string, string>();

export function mount(el: HTMLElement): void {
  el.style.height = "100%";
  el.innerHTML = `
    <div class="shot">
      <div class="shot-top">
        <button class="shot-capture" type="button">Capture area</button>
        <span class="shot-hint">or press <kbd>${shortcut}</kbd></span>
      </div>
      <p class="shot-error" role="alert" hidden></p>
      <div class="shot-grid"></div>
      <footer class="shot-foot">Drag a thumbnail to share · Kept 1 day</footer>
    </div>`;
  const grid = el.querySelector<HTMLElement>(".shot-grid")!;
  const error = el.querySelector<HTMLElement>(".shot-error")!;

  const showError = (e: unknown) => {
    error.textContent = String(e);
    error.hidden = false;
  };

  el.querySelector(".shot-capture")!.addEventListener("click", () => {
    error.hidden = true;
    invoke("screenshot_capture").catch(showError);
  });

  const refresh = () => render(grid).catch(showError);
  void listen("screenshot-changed", refresh);
  setInterval(refresh, 60_000); // keeps "time left" current
  void refresh();
}

async function render(grid: HTMLElement): Promise<void> {
  const shots = await invoke<Shot[]>("screenshot_list");
  if (shots.length === 0) {
    grid.innerHTML = `<p class="shot-empty">No screenshots yet.<br />Press <kbd>${shortcut}</kbd> or Capture area, then drag over what you want.</p>`;
  } else {
    const cards = await Promise.all(shots.map(card));
    grid.replaceChildren(...cards);
  }
  // Free thumbnails that are no longer listed.
  const keep = new Set(shots.map(key));
  for (const [k, url] of urls) {
    if (!keep.has(k)) {
      URL.revokeObjectURL(url);
      urls.delete(k);
    }
  }
}

const key = (s: Shot) => `${s.id}:${s.updatedAt}`;

async function thumbUrl(shot: Shot): Promise<string> {
  let url = urls.get(key(shot));
  if (!url) {
    const bytes = await invoke<ArrayBuffer>("screenshot_read", { id: shot.id });
    url = URL.createObjectURL(new Blob([bytes], { type: "image/png" }));
    urls.set(key(shot), url);
  }
  return url;
}

function timeLeft(shot: Shot): string {
  const ms = shot.createdAt + KEEP_MS - Date.now();
  const hours = Math.floor(ms / 3_600_000);
  return hours >= 1 ? `${hours}h` : `${Math.max(1, Math.floor(ms / 60_000))}m`;
}

async function card(shot: Shot): Promise<HTMLElement> {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "shot-card";
  button.title = "Click to edit, drag to share";
  const img = document.createElement("img");
  img.alt = "Screenshot";
  img.draggable = false;
  img.src = await thumbUrl(shot);
  const left = document.createElement("span");
  left.className = "shot-left";
  left.textContent = timeLeft(shot);
  button.append(img, left);

  // A click opens the editor; moving the pointer while pressed starts a native file drag.
  let from: { x: number; y: number } | null = null;
  let dragged = false;
  button.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    from = { x: e.clientX, y: e.clientY };
    dragged = false;
  });
  button.addEventListener("pointermove", (e) => {
    if (!from || !(e.buttons & 1)) return;
    if (Math.hypot(e.clientX - from.x, e.clientY - from.y) < 4) return;
    from = null;
    dragged = true;
    void startDrag({ item: [shot.path], icon: dragIcon(img) });
  });
  button.addEventListener("pointerup", () => (from = null));
  button.addEventListener("click", () => {
    if (!dragged) void invoke("screenshot_open", { id: shot.id });
  });
  return button;
}

/** A small PNG of the thumbnail for the drag preview (a full-size screenshot would cover the screen). */
function dragIcon(img: HTMLImageElement): string {
  const scale = Math.min(1, 128 / Math.max(img.naturalWidth, img.naturalHeight, 1));
  const c = document.createElement("canvas");
  c.width = Math.max(1, Math.round(img.naturalWidth * scale));
  c.height = Math.max(1, Math.round(img.naturalHeight * scale));
  c.getContext("2d")!.drawImage(img, 0, 0, c.width, c.height);
  return c.toDataURL("image/png");
}
