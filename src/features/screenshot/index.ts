// Screenshot feature. Called once, the first time the Screenshot tab is shown.
// Rust side: src-tauri/src/screenshot.rs. Editor window: editor.html + src/editor/.
import { exit } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { pngUrl } from "../../png-url";
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
  el.innerHTML = `
    <div class="toolbar shot-top">
      <button class="btn-primary shot-capture" type="button">Capture area</button>
      <p class="hint shot-hint">or press <kbd>${shortcut}</kbd></p>
    </div>
    <div class="shot-perm" role="alert" hidden>
      <strong>Allow screen recording</strong>
      <p class="hint">macOS needs your OK before HL Desk can capture other windows; without it a capture shows only your wallpaper. Turn on HL Desk in Privacy &amp; Security → Screen Recording, then quit and reopen HL Desk.</p>
      <div class="shot-perm-actions">
        <button class="btn shot-perm-open" type="button">Open System Settings</button>
        <button class="btn shot-perm-quit" type="button">Quit HL Desk</button>
      </div>
    </div>
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

  // macOS Screen Recording: the card shows while it is not allowed (only macOS asks), and when a
  // capture stopped because of it (Rust checks before each capture and sends this event).
  if (isMac) {
    invoke<boolean>("screenshot_permission")
      .then((granted) => (perm.hidden = granted))
      .catch(() => {});
  }
  void listen("screenshot-permission", () => {
    error.hidden = true;
    perm.hidden = false;
  });
  el.querySelector(".shot-perm-open")!.addEventListener("click", () => {
    invoke("screenshot_open_permission_settings").catch(showError);
  });
  el.querySelector(".shot-perm-quit")!.addEventListener("click", () => {
    exit(0).catch(() => {});
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
  setInterval(refresh, 60_000); // keeps "time left" current
  void refresh();
}

async function render(grid: HTMLElement, label: HTMLElement): Promise<void> {
  const shots = await invoke<Shot[]>("screenshot_list");
  label.hidden = shots.length === 0;
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
