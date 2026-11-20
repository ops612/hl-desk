// Corner toast (toast.html?kind=...), opened by src-tauri/src/screenshot/toast.rs. It never
// takes the keyboard. The window starts hidden on Windows: once the page is set up it calls
// screenshot_toast_show with its content height, and Rust sizes the window to it and shows it
// without taking the focus. Kinds:
// - cancelled: "Capture cancelled" for about 2 s.
// - copied: our own capture was copied to the clipboard; "Mark up" opens it in the editor
//   (editor-<id>), about 4 s.
// - shot: a device screenshot (Win+Shift+S, Cmd+Shift+4) with "Mark up in HL Desk", about 8 s.
// Each timer pauses while the pointer is over the toast.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import { invoke } from "@tauri-apps/api/core";
import { pngUrl } from "../png-url";
import "../styles.css";
import "./toast.css";
import { toastSetup } from "./kind";

const { kind, id, closeAfterMs } = toastSetup(location.search);
const NOTICES: Record<string, string> = { cancelled: "Capture cancelled" };

function log(level: "info" | "error", message: string): void {
  void invoke("app_log", { level, message: `toast: ${message}` }).catch(() => {});
}

const describe = (e: unknown) => (e instanceof Error ? e.message : String(e));
window.addEventListener("error", (e) => log("error", `script error (${kind}): ${describe(e.error ?? e.message)}`));
window.addEventListener("unhandledrejection", (e) => log("error", `unhandled promise (${kind}): ${describe(e.reason)}`));
log("info", `page loaded (${kind})`);

let closing = false;
function close(): void {
  if (closing) return;
  closing = true;
  void invoke("screenshot_toast_close").catch((e) => log("error", `close: ${String(e)}`));
}

/** Closes after `ms`, paused while the pointer is over the toast. */
function autoClose(ms: number): void {
  let timer = window.setTimeout(close, ms);
  document.body.addEventListener("pointerenter", () => clearTimeout(timer));
  document.body.addEventListener("pointerleave", () => {
    clearTimeout(timer);
    timer = window.setTimeout(close, 2500);
  });
}

if (kind === "shot") {
  const shot = document.getElementById("shot")!;
  const img = document.getElementById("thumb-img") as HTMLImageElement;
  const markup = document.getElementById("markup") as HTMLButtonElement;
  shot.hidden = false;
  const open = () => {
    markup.disabled = true;
    invoke("screenshot_toast_markup").catch((e) => {
      log("error", `mark up: ${String(e)}`);
      markup.disabled = false;
      markup.textContent = "Could not open it";
    });
  };
  markup.addEventListener("click", open);
  document.getElementById("thumb")!.addEventListener("click", open);
  document.getElementById("close")!.addEventListener("click", close);
  invoke<unknown>("screenshot_toast_image")
    .then((bytes) => (img.src = pngUrl(bytes)))
    .catch((e) => log("error", `thumbnail: ${String(e)}`));
  autoClose(closeAfterMs);
} else if (kind === "copied") {
  const copied = document.getElementById("copied")!;
  const img = document.getElementById("copied-img") as HTMLImageElement;
  const markup = document.getElementById("copied-markup") as HTMLButtonElement;
  copied.hidden = false;
  // The capture is saved already: Mark up opens its editor and closes the notice.
  const open = () => {
    markup.disabled = true;
    log("info", `mark up clicked for shot ${id}`);
    (id === null ? Promise.reject(new Error("no shot id")) : invoke("screenshot_open", { id }))
      .then(close)
      .catch((e) => {
        log("error", `mark up: ${String(e)}`);
        markup.disabled = false;
        markup.textContent = "Could not open it";
      });
  };
  markup.addEventListener("click", open);
  document.getElementById("copied-thumb")!.addEventListener("click", open);
  document.getElementById("copied-close")!.addEventListener("click", close);
  if (id !== null) {
    invoke<unknown>("screenshot_read", { id })
      .then((bytes) => (img.src = pngUrl(bytes)))
      .catch((e) => log("error", `thumbnail: ${String(e)}`));
  }
  autoClose(closeAfterMs);
} else {
  const notice = document.getElementById("notice")!;
  notice.textContent = NOTICES[kind] ?? kind;
  notice.hidden = false;
  autoClose(closeAfterMs);
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});

// Set up: now Rust may size the window to the content and show it (it closes a toast that never
// reports, so none stays blank). Height in CSS pixels: the content's plus the body's top and bottom
// border, measured once the fonts are in (they set the line height).
async function show(): Promise<void> {
  await document.fonts.ready.catch(() => {});
  const content = document.getElementById(kind === "shot" || kind === "copied" ? kind : "notice")!;
  const border = document.body.offsetHeight - document.body.clientHeight;
  const height = Math.ceil(content.getBoundingClientRect().height + border);
  await invoke("screenshot_toast_show", { height });
}
show().catch((e) => log("error", `show: ${describe(e)}`));
