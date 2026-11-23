// Pure parts of the editor's Copy text button and pen cursor (tested in copy-text.test.ts).

/** The paragraphs as plain text, one per line, empty ones left out. */
export function joinParagraphs(paragraphs: string[]): string {
  return paragraphs
    .map((p) => p.trim())
    .filter(Boolean)
    .join("\n");
}

/** Number of non-empty lines in `text`. */
export function countLines(text: string): number {
  return text.split("\n").filter((l) => l.trim()).length;
}

/** The button label after a copy: "Text copied · 3 lines", "· 1 line", or "No text found". */
export function copiedLabel(lines: number): string {
  if (lines <= 0) return "No text found";
  return `Text copied · ${lines} ${lines === 1 ? "line" : "lines"}`;
}

/** Shown instead of "No text found" when Windows has no Chinese recognizer (ocr-no-chinese). */
export const ONLY_ENGLISH = "Only English can be read. Add Chinese in Windows language settings.";

/** True for the OCR error that says no Chinese recognizer is installed (src-tauri/src/ocr.rs). */
export function isNoChineseError(message: string): boolean {
  return message.startsWith("ocr-no-chinese:");
}

/**
 * The pen cursor: a dot in `color`, `size` CSS pixels across (at least 6), with a thin white
 * outline, hotspot at the centre; crosshair where the image cursor is not supported.
 */
export function penCursor(color: string, size: number): string {
  const d = Math.max(6, Math.round(size));
  const box = d + 2; // 1 px white outline all round
  const c = box / 2;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" width="${box}" height="${box}" viewBox="0 0 ${box} ${box}">` +
    `<circle cx="${c}" cy="${c}" r="${d / 2}" fill="${color}" stroke="#fff" stroke-width="1"/></svg>`;
  const hot = Math.floor(c);
  return `url("data:image/svg+xml,${encodeURIComponent(svg)}") ${hot} ${hot}, crosshair`;
}
