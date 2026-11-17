// Corner toast (toast.html?kind=...), opened by src-tauri/src/screenshot/toast.rs. It never
// takes the keyboard. Kinds:
// - cancelled: "Capture cancelled" for about 2 s.
// - shot: a device screenshot (Win+Shift+S, Cmd+Shift+4) with "Mark up in HL Desk", about 6 s;
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
  autoClose(6000);
} else {
  const notice = document.getElementById("notice")!;
  notice.textContent = NOTICES[kind] ?? kind;
  notice.hidden = false;
  autoClose(2000);
}

document.addEventListener("keydown", (e) => {
  if (e.key === "Escape") close();
});
