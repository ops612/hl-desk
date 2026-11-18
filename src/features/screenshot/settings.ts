// Capture settings, shown at the top of the Settings sheet. The shell puts an empty
// <section id="settings-capture"> there and calls mountCaptureSettings(el) once.
// Rust side: screenshot_capture_settings / screenshot_set_capture_key / screenshot_set_markup /
// screenshot_open_key_settings in src-tauri/src/screenshot.rs (stored in settings.json).
import { invoke } from "@tauri-apps/api/core";
import { icon } from "../../icons";
import "./settings.css";

/** "off" | "ok" | "owned" (the system still uses the key) | "check" (could not tell) | "failed". */
type DeviceKey = "off" | "ok" | "owned" | "check" | "failed";
/** folderBlocked: macOS, mark up is on but the screenshot folder cannot be read. */
type CaptureSettings = { key: "default" | "device"; deviceKey: DeviceKey; markup: boolean; folderBlocked: boolean };

const isMac = navigator.userAgent.includes("Mac");
const DEFAULT_KEY = isMac ? "⌘⇧2" : "Ctrl+Shift+2";
const DEVICE_KEY = isMac ? "⌘⇧4" : "Print Screen";
const OPEN_LABEL = isMac ? "Open Keyboard Settings" : "Open keyboard settings";
const MARKUP_HINT = `After ${isMac ? "⌘⇧4" : "Win+Shift+S"}, a preview in the corner offers to open it in the editor.`;
/** macOS, while turning mark up on: the Desktop access prompt comes next. */
const MARKUP_ASKING = "macOS will ask to let HL Desk see your Desktop, where screenshots are saved. Click Allow.";

/** The one-line hint under the capture key, by state of the device key. */
function note(state: DeviceKey): string {
  if (isMac) {
    switch (state) {
      case "owned":
        return `macOS still uses ⌘⇧4 for its own screenshots. In Keyboard Shortcuts › Screenshots, turn off “Save picture of selected area as a file”. ${DEFAULT_KEY} works meanwhile.`;
      case "failed":
        return `⌘⇧4 could not be set up (another app may use it). ${DEFAULT_KEY} still works.`;
      case "check":
        return `If ⌘⇧4 still saves a file to the Desktop, turn it off in Keyboard Shortcuts › Screenshots.`;
      default:
        return `⌘⇧4 captures with HL Desk. ${DEFAULT_KEY} works too.`;
    }
  }
  switch (state) {
    case "owned":
      return `Windows still opens Snipping Tool with Print Screen. Turn off “Use the Print screen key to open screen capture”. ${DEFAULT_KEY} works meanwhile.`;
    case "failed":
      return `Print Screen could not be set up (Windows or another app, such as OneDrive, uses it). ${DEFAULT_KEY} still works.`;
    case "check":
      return `If Print Screen still opens Snipping Tool, turn off “Use the Print screen key to open screen capture”. ${DEFAULT_KEY} works too.`;
    default:
      return `Print Screen captures with HL Desk. ${DEFAULT_KEY} works too.`;
  }
}

export function mountCaptureSettings(el: HTMLElement): void {
  el.innerHTML = `
    <h3 class="sheet-section">${icon("crop")}Capture</h3>
    <div class="setting">
      <span>Capture key</span>
      <select class="select cap-key" aria-label="Capture key">
        <option value="default">${DEFAULT_KEY}</option>
        <option value="device">${DEVICE_KEY}</option>
      </select>
    </div>
    <div class="cap-note" hidden>
      <p class="hint cap-note-text"></p>
      <button class="btn cap-open" type="button">${OPEN_LABEL}</button>
    </div>
    <label class="setting">
      <span>Mark up in HL Desk after a device screenshot</span>
      <input class="switch cap-markup" type="checkbox" role="switch" />
    </label>
    <p class="hint cap-markup-hint">${MARKUP_HINT}</p>
    <div class="cap-note warn cap-folder" hidden>
      <p class="hint">HL Desk can't read your screenshots folder.</p>
      <button class="btn cap-folder-open" type="button">Open Privacy Settings</button>
    </div>
    <p class="setting-error cap-error" role="alert" hidden></p>`;

  const keySelect = el.querySelector<HTMLSelectElement>(".cap-key")!;
  const noteBox = el.querySelector<HTMLElement>(".cap-note")!;
  const noteText = el.querySelector<HTMLElement>(".cap-note-text")!;
  const openBtn = el.querySelector<HTMLButtonElement>(".cap-open")!;
  const markup = el.querySelector<HTMLInputElement>(".cap-markup")!;
  const markupHint = el.querySelector<HTMLElement>(".cap-markup-hint")!;
  const folderBox = el.querySelector<HTMLElement>(".cap-folder")!;
  const error = el.querySelector<HTMLElement>(".cap-error")!;

  const showError = (e: unknown) => {
    error.textContent = String(e);
    error.hidden = false;
  };

  const show = (s: CaptureSettings) => {
    error.hidden = true;
    keySelect.value = s.key;
    markup.checked = s.markup;
    markupHint.textContent = MARKUP_HINT;
    folderBox.hidden = !s.folderBlocked;
    noteBox.hidden = s.key !== "device";
    noteText.textContent = note(s.deviceKey);
    // The button only where the user has something to change in the system.
    openBtn.hidden = s.deviceKey === "ok";
    noteBox.classList.toggle("warn", s.deviceKey === "owned" || s.deviceKey === "failed");
  };

  const refresh = () => invoke<CaptureSettings>("screenshot_capture_settings").then(show).catch(showError);

  keySelect.addEventListener("change", () => {
    invoke<CaptureSettings>("screenshot_set_capture_key", { key: keySelect.value })
      .then(show)
      .catch((e) => {
        showError(e);
        void refresh();
      });
  });
  markup.addEventListener("change", () => {
    // Turning it on looks at the screenshot folder; macOS asks for Desktop access the first time.
    if (isMac && markup.checked) markupHint.textContent = MARKUP_ASKING;
    invoke<CaptureSettings>("screenshot_set_markup", { on: markup.checked })
      .then(show)
      .catch((e) => {
        showError(e);
        void refresh();
      });
  });
  openBtn.addEventListener("click", () => {
    invoke("screenshot_open_key_settings").catch(showError);
  });
  el.querySelector(".cap-folder-open")!.addEventListener("click", () => {
    invoke("screenshot_open_files_settings").catch(showError);
  });

  // The user may change the system setting and come back: check again whenever HL Desk is shown.
  window.addEventListener("focus", () => void refresh());
  void refresh();
}
