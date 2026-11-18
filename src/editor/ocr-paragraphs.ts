// Joins OCR lines that are one wrapped sentence back into one paragraph before translating, so
// "Windows doesn't hit the 'Could not check'" + "error now." is translated as one sentence.
// Lines arrive in reading order with pixel boxes (ocr.rs). A line starts a new paragraph when the
// previous one ends a sentence, when the vertical gap is large, when it starts further left than
// the paragraph, or when it starts with a bullet or a number.

export interface OcrLine {
  text: string;
  x: number;
  y: number;
  w: number;
  h: number;
}

const ENDS_SENTENCE = /[.!?:;。！？：；」』）)]["'”’]?$/;
const STARTS_ITEM = /^([•●▪◦·*\-–o]\s|\d{1,2}[.)]\s|[（(]?\d{1,2}[）)])/;
const CJK = /[　-ヿ㐀-鿿豈-﫿＀-￯]/;

function joinText(a: string, b: string): string {
  if (/[A-Za-z]-$/.test(a) && /^[a-z]/.test(b)) return a.slice(0, -1) + b;
  if (CJK.test(a.slice(-1)) || CJK.test(b.charAt(0))) return a + b;
  return `${a} ${b}`;
}

export function paragraphs(lines: OcrLine[]): string[] {
  const out: string[] = [];
  let text = "";
  let left = 0;
  let prev: OcrLine | null = null;
  for (const raw of lines) {
    const line = { ...raw, text: raw.text.trim() };
    if (!line.text) continue;
    const joins =
      prev !== null &&
      !ENDS_SENTENCE.test(text) &&
      !STARTS_ITEM.test(line.text) &&
      line.y - (prev.y + prev.h) <= Math.max(prev.h, line.h) * 0.9 &&
      line.x >= left - Math.max(prev.h, line.h);
    if (joins) {
      text = joinText(text, line.text);
    } else {
      if (text) out.push(text);
      text = line.text;
      left = line.x;
    }
    prev = line;
  }
  if (text) out.push(text);
  return out;
}
