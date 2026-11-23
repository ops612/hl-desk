// AI translation panel (the "AI" result under "Offline"): the pure part, tested by
// ai-logic.test.ts (npm test). The DOM and the Rust call are in ./ai.ts. Rust side:
// src-tauri/src/ai.rs (translate_ai), which calls the company Claude gateway.

/** Longer texts are not sent; as ai.rs MAX_CHARS (counted in characters, not UTF-16 units). */
export const AI_MAX_CHARS = 2_000;
export const AI_TOO_LONG = "Too long for AI";
export const AI_OFFLINE = "AI is offline right now";

export type AiTarget = "en" | "zh-Hant" | "zh-Hans";

/** What the panel shows. hidden: no panel at all (off, not built in, no text). */
export type AiView =
  | { kind: "hidden" }
  | { kind: "pending"; text: string }
  | { kind: "done"; text: string }
  | { kind: "error"; message: string };

export const charCount = (text: string) => [...text].length;

/**
 * One AI request per settled input (the caller's debounce decides when), latest wins: a reply
 * for older text or a cancelled run never overwrites a newer one. `call` makes the request
 * (Rust's translate_ai); `render` draws the panel.
 */
export class AiModel {
  /** Built with a gateway and switched on in Settings. */
  available = false;
  private n = 0;
  private last = "";
  /** Text and target of `last`: the same input again (a model download finished, a pick that
   * resolves to the same target) shows it again without a new request. */
  private lastKey = "";

  private readonly call: (text: string, target: AiTarget) => Promise<string>;
  private readonly render: (view: AiView) => void;

  constructor(call: (text: string, target: AiTarget) => Promise<string>, render: (view: AiView) => void) {
    this.call = call;
    this.render = render;
  }

  /** Translates `text` into `target` (null: no language resolved yet). */
  async run(text: string, target: AiTarget | null): Promise<void> {
    const mine = ++this.n;
    if (!this.available || !text.trim()) return this.hide();
    if (charCount(text) > AI_MAX_CHARS) {
      this.last = "";
      return this.render({ kind: "error", message: AI_TOO_LONG });
    }
    if (!target) return this.hide();
    const key = `${target}\n${text}`;
    if (this.last && key === this.lastKey) return this.render({ kind: "done", text: this.last });
    // The last AI result stays (muted) while the new one comes, as the offline result does.
    this.render({ kind: "pending", text: this.last || "Translating…" });
    try {
      const out = await this.call(text, target);
      if (mine !== this.n) return;
      this.last = out;
      this.lastKey = key;
      this.render({ kind: "done", text: out });
    } catch (e) {
      if (mine !== this.n) return;
      this.last = "";
      const message = typeof e === "string" && e ? e : AI_OFFLINE;
      this.render({ kind: "error", message });
    }
  }

  /** Drops any running request and hides the panel (window hidden, text cleared, switched off). */
  cancel(): void {
    this.n++;
    this.hide();
  }

  private hide(): void {
    this.last = "";
    this.render({ kind: "hidden" });
  }
}
