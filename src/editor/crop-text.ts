// Keeps only the OCR text inside the editor's crop, for Copy text and Translate. OCR reads the
// capture file on disk, which knows nothing of a crop not yet saved. A word stays when the centre
// of its box lies inside the crop, so a word cut in half goes with its larger part, and a line
// cut through its middle keeps just the words inside. Lines without word boxes use the line's box.

import type { OcrLine, OcrWord } from "./ocr-paragraphs";

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** The applied crop and where the file on disk starts, both in image pixels (main.ts). */
export interface TextRegion {
  crop: Rect;
  origin: { x: number; y: number };
}

const CJK = /[　-〿㐀-䶿一-鿿豈-﫿＀-￯]|[\u{20000}-\u{2FFFF}]/u;

/** True when the centre of `box` lies inside `crop` (edges included). */
export function centreInside(box: Rect, crop: Rect): boolean {
  const cx = box.x + box.w / 2;
  const cy = box.y + box.h / 2;
  return cx >= crop.x && cx <= crop.x + crop.w && cy >= crop.y && cy <= crop.y + crop.h;
}

/** Words to a line: a space between words, none next to a Chinese character (as ocr.rs). */
export function joinWords(words: string[]): string {
  let out = "";
  for (const word of words) {
    if (!word) continue;
    if (out && !CJK.test([...out].pop()!) && !CJK.test(word.charAt(0))) out += " ";
    out += word;
  }
  return out;
}

/**
 * The lines inside `crop` (image pixels of the capture as first opened). `origin` is where the
 * file that was read starts in those pixels: after a save the file on disk is the crop itself,
 * so its boxes are shifted by the saved crop's corner.
 */
export function linesInCrop(lines: OcrLine[], crop: Rect, origin = { x: 0, y: 0 }): OcrLine[] {
  const shift = <T extends OcrWord>(b: T): T => ({ ...b, x: b.x + origin.x, y: b.y + origin.y });
  const out: OcrLine[] = [];
  for (const raw of lines) {
    const line = shift(raw);
    const words = raw.words?.map(shift);
    if (!words?.length) {
      if (centreInside(line, crop)) out.push({ ...line, words });
      continue;
    }
    const kept = words.filter((w) => centreInside(w, crop));
    if (!kept.length) continue;
    if (kept.length === words.length) {
      out.push({ ...line, words });
      continue;
    }
    const x = Math.min(...kept.map((w) => w.x));
    const y = Math.min(...kept.map((w) => w.y));
    const w = Math.max(...kept.map((k) => k.x + k.w)) - x;
    const h = Math.max(...kept.map((k) => k.y + k.h)) - y;
    out.push({ text: joinWords(kept.map((k) => k.text)), x, y, w, h, words: kept });
  }
  return out;
}
