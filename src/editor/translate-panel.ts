// Screenshot editor: the Translate button and its right-side panel. Reads the capture's text
// (ocr_capture, src-tauri/src/ocr.rs), picks the language pair like the translate card
// (translate_detect: auto-detect plus the last manual pick) and translates every line with the
// shared engine (src/translate/engine.ts). Each line shows the original (muted) and its
// translation; "Copy translation" copies the translated lines, one per line. A missing pair gets
// the same first-use download block as the card. While the panel is open its Copy translation is
// the view's one indigo button, so the editor's Done turns plain.
import { invoke } from "@tauri-apps/api/core";
import {
  PAIRS,
  type PairId,
  type PairStatus,
  type Script,
  copyTranslation,
  describe,
  detectPair,
  downloadPair,
  downloadTexts,
  log,
  pairInfo,
  pairStatus,
  rememberPick,
  touchIdle,
  translateTexts,
} from "../translate/engine";
import { linesInCrop, type TextRegion } from "./crop-text";
import { paragraphs, type OcrLine } from "./ocr-paragraphs";

interface OcrText {
  lines: OcrLine[];
  language: string;
}

/** Wires the panel. `onLayout` runs after it opens or closes (the canvas refits). */
export function setupTranslatePanel(captureId: number, region: () => TextRegion, onLayout: () => void): void {
  const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
  const button = $<HTMLButtonElement>("translate");
  const panel = $("tr-panel");
  const done = $<HTMLButtonElement>("done");
  const langBtn = $<HTMLButtonElement>("tr-lang");
  const langLabel = $("tr-lang-label");
  const menu = $("tr-menu");
  const stateEl = $("tr-state");
  const getEl = $("tr-get");
  const getTitle = $("tr-get-title");
  const getText = $("tr-get-text");
  const progress = $("tr-progress");
  const progressBar = $("tr-progress-bar");
  const getStatus = $("tr-get-status");
  const downloadBtn = $<HTMLButtonElement>("tr-download");
  const errorEl = $("tr-error");
  const linesEl = $("tr-lines");
  const copyBtn = $<HTMLButtonElement>("tr-copy");

  let ocr: { lines: OcrLine[]; origin: TextRegion["origin"] } | null = null; // read once per editor
  let lines: string[] | null = null; // paragraphs inside the crop, from `ocr`
  let translations: string[] = [];
  let pair: PairId | null = null;
  let script: Script = "han";
  let token = 0;
  let pendingStatus: PairStatus | null = null;

  function setState(text: string): void {
    stateEl.textContent = text;
    stateEl.hidden = !text;
  }

  function showError(text: string): void {
    errorEl.textContent = text;
    errorEl.hidden = false;
  }

  function reset(): void {
    setState("");
    errorEl.hidden = true;
    getEl.hidden = true;
    linesEl.replaceChildren();
    copyBtn.disabled = true;
    translations = [];
  }

  function renderLang(): void {
    langLabel.textContent = pair ? pairInfo(pair).label : "Language";
    langBtn.disabled = !pair;
  }

  function renderLines(source: string[], target: string[] | null): void {
    linesEl.replaceChildren(
      ...source.map((text, i) => {
        const li = document.createElement("li");
        const orig = document.createElement("p");
        orig.className = "tr-line-source";
        orig.textContent = text;
        li.append(orig);
        if (target) {
          const tr = document.createElement("p");
          tr.className = "tr-line-result";
          tr.textContent = target[i] ?? "";
          li.append(tr);
        }
        return li;
      }),
    );
  }

  async function read(): Promise<void> {
    const mine = ++token;
    reset();
    setState("Reading text…");
    try {
      const { origin } = region();
      const out = await invoke<OcrText>("ocr_capture", { id: captureId });
      if (mine !== token) return;
      ocr = { lines: out.lines, origin };
      log("info", `editor: OCR found ${out.lines.length} lines (${out.language})`);
    } catch (e) {
      if (mine !== token) return;
      // The error is a sentence for the user; one kind starts with "ocr-no-chinese:".
      const text = describe(e).replace(/^ocr-no-chinese:\s*/, "");
      log("error", `editor: OCR failed: ${text}`);
      setState("");
      showError(text);
      return;
    }
    await show();
  }

  /** The paragraphs inside the current crop. */
  function cropped(): string[] {
    return ocr ? paragraphs(linesInCrop(ocr.lines, region().crop, ocr.origin)) : [];
  }

  /** Shows the text inside the crop and translates it. */
  async function show(): Promise<void> {
    const mine = ++token;
    reset();
    lines = cropped();
    log("info", `editor: ${lines.length} paragraphs inside the crop`);
    if (!lines.length) {
      setState("No text found in this screenshot.");
      return;
    }
    const found = await detectPair(lines.join("\n")).catch((e) => {
      log("error", `editor: detect: ${describe(e)}`);
      return null;
    });
    if (mine !== token) return;
    if (!found) {
      setState("No Chinese or English text found in this screenshot.");
      renderLines(lines, null);
      return;
    }
    pair = found.pair;
    script = found.script;
    renderLang();
    await translate();
  }

  async function translate(): Promise<void> {
    if (!lines?.length || !pair) return;
    const mine = ++token;
    const source = lines;
    reset();
    renderLines(source, null);
    let status: PairStatus;
    try {
      status = await pairStatus(pair);
    } catch (e) {
      showError(`Could not check the language files: ${describe(e)}`);
      return;
    }
    if (mine !== token) return;
    if (!status.installed) {
      offerDownload(status);
      return;
    }
    setState("Translating…");
    try {
      const out = await translateTexts(status, source);
      if (mine !== token) return;
      translations = out;
      setState("");
      renderLines(source, out);
      copyBtn.disabled = false;
    } catch (e) {
      if (mine !== token) return;
      log("error", `editor: translation failed (${pair}): ${describe(e)}`);
      setState("");
      showError(`Could not translate: ${describe(e)}`);
    }
  }

  function offerDownload(status: PairStatus): void {
    pendingStatus = status;
    const texts = downloadTexts(status);
    getEl.hidden = false;
    getTitle.textContent = texts.title;
    getText.textContent = texts.text;
    progress.hidden = true;
    getStatus.hidden = true;
    downloadBtn.hidden = false;
    downloadBtn.textContent = "Download";
  }

  async function download(): Promise<void> {
    const status = pendingStatus;
    if (!status) return;
    // A language pick while this downloads starts a new run: then this result is stale.
    const mine = token;
    errorEl.hidden = true;
    downloadBtn.hidden = true;
    progress.hidden = false;
    getStatus.hidden = false;
    const texts = downloadTexts(status);
    const show = (n: number) => {
      if (mine !== token) return;
      progressBar.style.width = `${Math.min(100, (n / status.totalBytes) * 100)}%`;
      getStatus.textContent = texts.progress(n);
    };
    show(0);
    try {
      await downloadPair(status, show);
      if (mine === token && pair === status.pair) await translate();
    } catch (e) {
      log("error", `editor: download of ${status.pair} failed: ${describe(e)}`);
      if (mine !== token) return;
      progress.hidden = true;
      getStatus.hidden = true;
      downloadBtn.hidden = false;
      downloadBtn.textContent = "Retry";
      showError(describe(e));
    }
  }

  function open(): void {
    panel.hidden = false;
    button.setAttribute("aria-pressed", "true");
    // One indigo button per view: Copy translation while the panel is open.
    done.classList.replace("primary", "plain");
    onLayout();
    log("info", "editor: translate panel opened");
    if (ocr === null) void read();
    // Cropped since the last read: show only what is inside the new crop.
    else if (cropped().join("\n") !== lines?.join("\n")) void show();
  }

  function close(): void {
    closeMenu();
    panel.hidden = true;
    button.setAttribute("aria-pressed", "false");
    done.classList.replace("plain", "primary");
    onLayout();
    touchIdle();
  }

  function closeMenu(): void {
    menu.hidden = true;
    langBtn.setAttribute("aria-expanded", "false");
  }

  function openMenu(): void {
    menu.replaceChildren(
      ...PAIRS.map((p) => {
        const item = document.createElement("button");
        item.type = "button";
        item.className = "row";
        item.setAttribute("role", "menuitemradio");
        item.setAttribute("aria-checked", String(p.id === pair));
        item.textContent = p.label;
        item.addEventListener("click", () => {
          closeMenu();
          if (p.id === pair) return;
          pair = p.id;
          renderLang();
          log("info", `editor: picked ${p.id} by hand`);
          rememberPick(script, p.id);
          void translate();
        });
        return item;
      }),
    );
    menu.hidden = false;
    langBtn.setAttribute("aria-expanded", "true");
  }

  button.addEventListener("click", () => (panel.hidden ? open() : close()));
  $("tr-close").addEventListener("click", close);
  langBtn.addEventListener("click", () => (menu.hidden ? openMenu() : closeMenu()));
  document.addEventListener("pointerdown", (e) => {
    if (!menu.hidden && !(e.target as Element).closest(".tr-picker")) closeMenu();
  });
  downloadBtn.addEventListener("click", () => void download());
  copyBtn.addEventListener("click", async () => {
    if (!translations.length) return;
    try {
      await copyTranslation(translations.join("\n"));
      copyBtn.textContent = "Copied";
      window.setTimeout(() => (copyBtn.textContent = "Copy translation"), 1200);
    } catch (e) {
      showError(describe(e));
    }
  });
  renderLang();
}
