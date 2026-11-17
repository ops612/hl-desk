// Corner toast (toast.html?kind=...), opened by src-tauri/src/screenshot/toast.rs. It never
// takes the keyboard. The window starts hidden on Windows: once the page is set up it calls
// screenshot_toast_show, and Rust shows it without taking the focus. Kinds:
// - cancelled: "Capture cancelled" for about 2 s.
// - shot: a device screenshot (Win+Shift+S, Cmd+Shift+4) with "Mark up in HL Desk", about 8 s;
//   the timer pauses while the pointer is over it.
import "@fontsource/ibm-plex-sans/400.css";
import "@fontsource/ibm-plex-sans/500.css";
import { invoke } from "@tauri-apps/api/core";
import { pngUrl } from "../png-url";
import "../styles.css";
import "./toast.css";

const kind = new URLSearchParams(location.search).get("kind") ?? "";
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
  autoClose(8000);
} else {
  const notice = document.getElementById("notice")!;
  notice.textContent = NOTICES[kind] ?? kind;
  notice.hidden = false;
  autoClose(2000);
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});

// Set up: now Rust may show the window (it closes a toast that never reports, so none stays blank).
invoke("screenshot_toast_show").catch((e) => log("error", `show: ${describe(e)}`));
