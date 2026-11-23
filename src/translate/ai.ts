// AI translation panel for the Translate window (translator.html) and the translate card
// (translate.html): the panel markup is in each page (#ai, #ai-out, #ai-error, #ai-copy); this
// binds it. The request goes through Rust (src-tauri/src/ai.rs, translate_ai), so the gateway
// token never reaches this page. Pure part (latest wins, length limit): ./ai-logic.ts.
import { invoke } from "@tauri-apps/api/core";
import { AiModel, type AiTarget, type AiView } from "./ai-logic";
import { copyTranslation, describe, log } from "./engine";

export type { AiTarget } from "./ai-logic";

interface AiStatus {
  available: boolean;
  enabled: boolean;
}

export interface AiPanel {
  /** Re-reads whether AI is built in and switched on; hides the panel when it is not. */
  refresh(): Promise<void>;
  run(text: string, target: AiTarget | null): void;
  cancel(): void;
}

/** `onChange` runs after every redraw (the card re-measures its height); `onCopied` after Copy. */
export function mountAiPanel(opts: { onChange?: () => void; onCopied?: () => void } = {}): AiPanel {
  const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  const panel = $("ai");
  const out = $("ai-out");
  const error = $("ai-error");
  const copyBtn = $<HTMLButtonElement>("ai-copy");
  let shown = "";
  let copiedTimer = 0;

  const render = (view: AiView) => {
    clearTimeout(copiedTimer);
    copyBtn.textContent = "Copy";
    panel.hidden = view.kind === "hidden";
    const text = view.kind === "pending" || view.kind === "done" ? view.text : "";
    out.textContent = text;
    out.classList.toggle("pending", view.kind === "pending");
    error.hidden = view.kind !== "error";
    error.textContent = view.kind === "error" ? view.message : "";
    shown = view.kind === "done" ? view.text : "";
    copyBtn.disabled = !shown;
    opts.onChange?.();
  };

  const model = new AiModel((text, target) => invoke<string>("translate_ai", { text, target }), render);

  copyBtn.addEventListener("click", async () => {
    if (!shown) return;
    try {
      // translate_copy: the double-copy listener skips this write, so the card does not pop up.
      await copyTranslation(shown);
      log("info", "AI result copied");
      copyBtn.textContent = "Copied";
      copiedTimer = window.setTimeout(() => (copyBtn.textContent = "Copy"), 1200);
      opts.onCopied?.();
    } catch (e) {
      log("error", `AI copy: ${describe(e)}`);
    }
  });

  return {
    async refresh() {
      try {
        const s = await invoke<AiStatus>("translate_ai_status");
        const on = s.available && s.enabled;
        if (model.available && !on) model.cancel();
        model.available = on;
      } catch (e) {
        log("error", `AI status: ${describe(e)}`);
      }
    },
    run: (text, target) => void model.run(text, target),
    cancel: () => model.cancel(),
  };
}
