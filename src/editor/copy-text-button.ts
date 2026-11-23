// Screenshot editor: the Copy text button. Reads the capture's text with the same OCR as the
// Translate panel (ocr_capture, src-tauri/src/ocr.rs), keeps the words inside the crop, joins wrapped lines into paragraphs and
// copies them as plain text, one paragraph per line. The copy goes through translate_copy, which
// marks it as our own write so the double-copy translate card does not open for it.
import { invoke } from "@tauri-apps/api/core";
import { copyTranslation, describe, log } from "../translate/engine";
import { ONLY_ENGLISH, copiedLabel, countLines, isNoChineseError, joinParagraphs } from "./copy-text";
import { linesInCrop, type TextRegion } from "./crop-text";
import { paragraphs, type OcrLine } from "./ocr-paragraphs";

const LABEL = "Copy text";
const SHOW_MS = 2000;

/** Wires the button; returns the action, also used by the T key. */
export function setupCopyText(captureId: number, region: () => TextRegion): () => void {
  const button = document.getElementById("copy-text") as HTMLButtonElement;
  const label = document.getElementById("copy-text-label")!;
  const icon = document.getElementById("copy-text-icon")!;
  const check = document.getElementById("copy-text-check")!;
  let busy = false;
  let timer = 0;

  function show(text: string, ok: boolean): void {
    label.textContent = text;
    icon.style.display = ok ? "none" : "";
    check.style.display = ok ? "" : "none";
  }

  async function copyText(): Promise<void> {
    if (busy) return;
    busy = true;
    clearTimeout(timer);
    button.disabled = true;
    show("Reading text…", false);
    let message: string;
    let ok = false;
    let read = false;
    try {
      const { origin } = region(); // the file as it is now; a save during OCR is not expected
      const out = await invoke<{ lines: OcrLine[]; language: string }>("ocr_capture", { id: captureId });
      read = true;
      const text = joinParagraphs(paragraphs(linesInCrop(out.lines, region().crop, origin)));
      const n = countLines(text);
      if (n) {
        await copyTranslation(text);
        ok = true;
        log("info", `editor: copy text, ${text.length} chars, ${n} lines (${out.language})`);
      } else {
        log("info", `editor: copy text, no text found (${out.language})`);
      }
      message = copiedLabel(n);
    } catch (e) {
      const why = describe(e);
      log("error", `editor: copy text failed: ${why}`);
      // Read but not copied: the clipboard refused ("Could not copy: …").
      message = read ? why : isNoChineseError(why) ? ONLY_ENGLISH : copiedLabel(0);
    }
    show(message, ok);
    button.disabled = false;
    busy = false;
    timer = window.setTimeout(() => show(LABEL, false), SHOW_MS);
  }

  button.addEventListener("click", () => void copyText());
  return () => void copyText();
}
