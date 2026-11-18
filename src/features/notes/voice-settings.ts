// Settings > Voice to text (Windows only; hidden where voice_status says unsupported).
// The shell puts an empty <section id="settings-voice"> in the Settings sheet and calls
// mountVoiceSettings(el) once. Rust side: voice_status / voice_set_enabled / voice_download in
// src-tauri/src/voice.rs (the switch is stored in settings.json). Notes uses it: index.ts.
import { Channel, invoke } from "@tauri-apps/api/core";
import { icon } from "../../icons";

type VoiceStatus = {
  supported: boolean;
  /** Why this PC can't run it (no AVX), or null. */
  unsupportedReason: string | null;
  enabled: boolean;
  installed: boolean;
  downloading: boolean;
  totalBytes: number;
};

const MB = 1_000_000;
const HINT = "English. Downloads 148 MB once, then works without internet.";

function log(level: "info" | "warn" | "error", message: string): void {
  invoke("app_log", { level, message: `voice settings: ${message}` }).catch(() => {});
}

export function mountVoiceSettings(el: HTMLElement): void {
  el.hidden = true;
  el.innerHTML = `
    <h3 class="sheet-section">${icon("mic")}Voice to text</h3>
    <div class="setting">
      <span class="setting-label">Offline mode<span class="hint voice-hint" role="status">${HINT}</span></span>
      <button class="btn voice-retry" type="button" hidden>Retry</button>
      <input class="switch voice-switch" type="checkbox" role="switch" aria-label="Offline mode" />
    </div>
    <p class="setting-error voice-error" role="alert" hidden></p>`;

  const sw = el.querySelector<HTMLInputElement>(".voice-switch")!;
  const hint = el.querySelector<HTMLElement>(".voice-hint")!;
  const retry = el.querySelector<HTMLButtonElement>(".voice-retry")!;
  const error = el.querySelector<HTMLElement>(".voice-error")!;
  let downloading = false;

  const showError = (message: string | null) => {
    error.textContent = message ?? "";
    error.hidden = message === null;
    retry.hidden = message === null;
  };

  async function download(): Promise<void> {
    if (downloading) return;
    downloading = true;
    showError(null);
    let total = 148 * MB;
    hint.textContent = "Downloading… 0 of 148 MB";
    const channel = new Channel<{ done: number; total: number }>();
    channel.onmessage = (p) => {
      total = p.total;
      hint.textContent = `Downloading… ${Math.floor(p.done / MB)} of ${Math.round(total / MB)} MB`;
    };
    log("info", "model download started");
    try {
      await invoke("voice_download", { onProgress: channel });
      log("info", "model downloaded");
      // Turned off while it downloaded: no "works without internet" line while off.
      hint.textContent = sw.checked ? "Downloaded. Works without internet." : HINT;
    } catch (e) {
      log("error", `model download: ${e}`);
      hint.textContent = HINT;
      showError(String(e));
    } finally {
      downloading = false;
    }
  }

  function show(s: VoiceStatus): void {
    el.hidden = !s.supported;
    sw.checked = s.enabled;
    // This PC's processor can't run it: the switch stays off, with the reason.
    sw.disabled = s.unsupportedReason !== null;
    if (s.unsupportedReason !== null) {
      hint.textContent = s.unsupportedReason;
      showError(null);
      return;
    }
    if (downloading) return;
    if (s.enabled && !s.installed && !s.downloading) {
      hint.textContent = HINT;
      showError("The voice model isn't downloaded yet.");
    } else if (!s.enabled) {
      hint.textContent = HINT;
      showError(null);
    }
  }

  const refresh = () =>
    invoke<VoiceStatus>("voice_status")
      .then(show)
      .catch(() => {});

  sw.addEventListener("change", async () => {
    const want = sw.checked;
    showError(null);
    try {
      const s = await invoke<VoiceStatus>("voice_set_enabled", { on: want });
      log("info", `turned ${want ? "on" : "off"}`);
      show(s);
      if (want && !s.installed) await download();
    } catch (e) {
      sw.checked = !want;
      showError(String(e));
      retry.hidden = true;
    }
  });
  retry.addEventListener("click", () => void download());

  // Settings may be opened again later: show the current state whenever HL Desk is shown.
  window.addEventListener("focus", () => void refresh());
  void refresh();
}
