// Settings > Translate > "AI translation (online)" (on by default). The shell puts an empty
// <section id="settings-translate"> in the Settings sheet and calls mountAiSettings(el) once.
// Hidden when HL Desk was built without the company gateway. Rust side: translate_ai_status /
// translate_ai_set_enabled in src-tauri/src/ai.rs (stored in settings.json).
import { invoke } from "@tauri-apps/api/core";
import { icon } from "../icons";

type AiStatus = { available: boolean; enabled: boolean };

const HINT = "A second result under Offline. Sends the text to the company AI service.";

function log(level: "info" | "warn" | "error", message: string): void {
  invoke("app_log", { level, message: `ai settings: ${message}` }).catch(() => {});
}

export function mountAiSettings(el: HTMLElement): void {
  el.hidden = true;
  el.innerHTML = `
    <h3 class="sheet-section">${icon("globe")}Translate</h3>
    <label class="setting">
      <span class="setting-label">AI translation (online)<span class="hint">${HINT}</span></span>
      <input class="switch ai-switch" type="checkbox" role="switch" aria-label="AI translation (online)" />
    </label>
    <p class="setting-error ai-error" role="alert" hidden></p>`;
  const sw = el.querySelector<HTMLInputElement>(".ai-switch")!;
  const error = el.querySelector<HTMLElement>(".ai-error")!;

  const show = (s: AiStatus) => {
    el.hidden = !s.available;
    sw.checked = s.enabled;
  };
  const refresh = () =>
    invoke<AiStatus>("translate_ai_status")
      .then(show)
      .catch(() => {});

  sw.addEventListener("change", async () => {
    const want = sw.checked;
    error.hidden = true;
    try {
      show(await invoke<AiStatus>("translate_ai_set_enabled", { on: want }));
      log("info", `turned ${want ? "on" : "off"}`);
    } catch (e) {
      sw.checked = !want;
      error.textContent = String(e);
      error.hidden = false;
    }
  });

  window.addEventListener("focus", () => void refresh());
  void refresh();
}
