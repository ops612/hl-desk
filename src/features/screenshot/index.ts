// Screenshot feature. Called once, the first time the Screenshot tab is shown.
// Rust side: src-tauri/src/screenshot.rs. Editor window: editor.html + src/editor/.
import { invoke } from "@tauri-apps/api/core";
import { pngUrl } from "../../png-url";
import { listen } from "@tauri-apps/api/event";
import { startDrag } from "@crabnebula/tauri-plugin-drag";
import { icon } from "../../icons";
import { mountPermission } from "./permission";
import "./screenshot.css";

type Shot = { id: number; path: string; createdAt: number; updatedAt: number };

const KEEP_MS = 24 * 60 * 60 * 1000;
const isMac = navigator.userAgent.includes("Mac");
const DEFAULT_KEY = isMac ? "⌘⇧2" : "Ctrl+Shift+2";
const DEVICE_KEY = isMac ? "⌘⇧4" : "Print Screen";
/** The capture keys as shown on the tab (the device key is added when chosen in Settings). */
let keys = `<kbd>${DEFAULT_KEY}</kbd>`;

/** One key combination as keycaps: "Ctrl+Shift+2" as three caps, "⌘⇧2" as three. */
function keycaps(combo: string): string {
  const parts = isMac && !combo.includes(" ") ? [...combo] : combo.split("+");
  return parts.map((k) => `<kbd class="keycap">${k}</kbd>`).join("");
}
/** The capture card's keys: the configured one(s), as the old "or press" line had them. */
const cardKeys = (device: boolean) =>
  device ? `${keycaps(DEVICE_KEY)}<span class="shot-or">or</span>${keycaps(DEFAULT_KEY)}` : keycaps(DEFAULT_KEY);

// Thumbnail object URLs, keyed by "id:updatedAt" so an edited capture reloads.
const urls = new Map<string, string>();

export function mount(el: HTMLElement): void {
  el.innerHTML = `
    <div class="shot-card-capture">
      <span class="shot-tile">${icon("crop", 18)}</span>
      <p class="shot-title">Capture an area</p>
      <p class="hint shot-sub">Mark it up, translate it, drag it anywhere</p>
      <div class="shot-row">
        <button class="btn-primary shot-capture" type="button">Capture</button>
        <span class="shot-keys" aria-label="Shortcut">${cardKeys(false)}</span>
      </div>
    </div>
    <div class="shot-perm" role="alert" hidden></div>
    <p class="shot-error" role="alert" hidden></p>
    <div class="scroll">
      <div class="hint shot-label" hidden>Recent</div>
      <div class="shot-grid"></div>
    </div>
    <footer class="footer"><span>Drag a thumbnail to share</span><span>Kept 1 day</span></footer>`;
  const grid = el.querySelector<HTMLElement>(".shot-grid")!;
  const label = el.querySelector<HTMLElement>(".shot-label")!;
  const error = el.querySelector<HTMLElement>(".shot-error")!;
  const perm = el.querySelector<HTMLElement>(".shot-perm")!;

  const showError = (e: unknown) => {
    error.textContent = String(e);
    error.hidden = false;
  };

  el.querySelector(".shot-capture")!.addEventListener("click", () => {
    error.hidden = true;
    invoke("screenshot_capture").catch(showError);
  });

  // macOS Screen Recording (permission.ts): the card shows while it is not allowed or needs a
  // restart (only macOS asks), and when a capture stopped because of it (Rust checks before each
  // capture and sends this event).
  const permission = mountPermission(perm);
  if (isMac) permission.check();
  void listen("screenshot-permission", () => {
    error.hidden = true;
    permission.check();
  });

  // A failed capture (Rust shows this window and sends the message). One sent before this tab
  // was first opened is picked up with screenshot_last_error.
  const showFailure = (message: string) => showError(`Capture failed. ${message}`);
  void listen<string>("screenshot-error", (e) => showFailure(e.payload));
  invoke<string | null>("screenshot_last_error")
    .then((message) => message && showFailure(message))
    .catch(() => {});

  const refresh = () => render(grid, label).catch(showError);
  void listen("screenshot-changed", refresh);

  // The capture keys (Settings > Capture key).
  const keysEl = el.querySelector<HTMLElement>(".shot-keys")!;
  const loadKeys = () =>
    invoke<{ key: string }>("screenshot_capture_settings")
      .then((s) => {
        keys =
          s.key === "device"
            ? `<kbd>${DEVICE_KEY}</kbd> or <kbd>${DEFAULT_KEY}</kbd>`
            : `<kbd>${DEFAULT_KEY}</kbd>`;
        keysEl.innerHTML = cardKeys(s.key === "device");
        void refresh();
      })
      .catch(() => {});
  void listen("screenshot-keys-changed", loadKeys);
  void loadKeys();
  setInterval(refresh, 60_000); // keeps "time left" current
  void refresh();
}

async function render(grid: HTMLElement, label: HTMLElement): Promise<void> {
  const shots = await invoke<Shot[]>("screenshot_list");
  label.hidden = shots.length === 0;
  if (shots.length === 0) {
    grid.innerHTML = `<p class="shot-empty">No screenshots yet.<br />Press ${keys} or Capture, then drag over what you want.</p>`;
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
    const bytes = await invoke<unknown>("screenshot_read", { id: shot.id });
    url = pngUrl(bytes);
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
