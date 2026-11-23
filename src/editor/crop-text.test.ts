// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import { centreInside, joinWords, linesInCrop } from "./crop-text.ts";
import type { OcrLine, OcrWord } from "./ocr-paragraphs.ts";
import { paragraphs } from "./ocr-paragraphs.ts";

const word = (text: string, x: number, y = 100, w = 40, h = 20): OcrWord => ({ text, x, y, w, h });
/** A line from its words, box = union, text joined like ocr.rs. */
function line(words: OcrWord[], text = joinWords(words.map((w) => w.text))): OcrLine {
  const x = Math.min(...words.map((w) => w.x));
  const y = Math.min(...words.map((w) => w.y));
  return {
    text,
    x,
    y,
    w: Math.max(...words.map((w) => w.x + w.w)) - x,
    h: Math.max(...words.map((w) => w.y + w.h)) - y,
    words,
  };
}
const texts = (lines: OcrLine[]) => lines.map((l) => l.text);

test("centre rule: inside, outside, straddling an edge", () => {
  const crop = { x: 100, y: 100, w: 200, h: 100 };
  assert.ok(centreInside({ x: 120, y: 120, w: 40, h: 20 }, crop)); // fully inside
  assert.ok(!centreInside({ x: 400, y: 120, w: 40, h: 20 }, crop)); // fully outside
  assert.ok(centreInside({ x: 80, y: 120, w: 60, h: 20 }, crop)); // centre 110: mostly inside
  assert.ok(!centreInside({ x: 60, y: 120, w: 60, h: 20 }, crop)); // centre 90: mostly outside
  assert.ok(centreInside({ x: 90, y: 120, w: 20, h: 20 }, crop)); // centre on the edge counts
});

test("no crop applied keeps every line unchanged", () => {
  const lines = [line([word("Hello", 10), word("world", 60)]), line([word("Next", 10, 140)])];
  const out = linesInCrop(lines, { x: 0, y: 0, w: 1000, h: 1000 });
  assert.deepEqual(texts(out), ["Hello world", "Next"]);
  assert.equal(out[0].text, lines[0].text);
});

test("a line cut in half keeps only its words inside the crop", () => {
  const l = line([word("The", 10), word("quick", 60), word("brown", 110), word("fox.", 160)]);
  // Crop starts at x=100: "The" (centre 30) and "quick" (centre 80) are out.
  const out = linesInCrop([l], { x: 100, y: 0, w: 500, h: 500 });
  assert.deepEqual(texts(out), ["brown fox."]);
  assert.equal(out[0].x, 110);
  assert.equal(out[0].w, 90);
});

test("a word cut in half goes with its larger part", () => {
  const l = line([word("alpha", 10, 100, 80), word("beta", 100, 100, 80)]);
  // Edge at x=60: "alpha" centre 50 is outside, though 30 px of it are inside.
  assert.deepEqual(texts(linesInCrop([l], { x: 60, y: 0, w: 500, h: 500 })), ["beta"]);
  // Edge at x=40: "alpha" centre 50 is inside, so the whole word stays.
  assert.deepEqual(texts(linesInCrop([l], { x: 40, y: 0, w: 500, h: 500 })), ["alpha beta"]);
});

test("a line with only its top sliver inside is dropped", () => {
  const top = line([word("kept", 10, 100)]);
  const sliver = line([word("cut", 10, 140), word("off", 60, 140)]); // y 140..160, centre 150
  const out = linesInCrop([top, sliver], { x: 0, y: 90, w: 500, h: 55 }); // bottom edge 145
  assert.deepEqual(texts(out), ["kept"]);
});

test("Chinese words join with no spaces, mixed text keeps spaces between Latin words", () => {
  const zh = line([word("你", 10, 100, 20), word("好", 30, 100, 20), word("世", 50, 100, 20), word("界", 70, 100, 20)]);
  assert.deepEqual(texts(linesInCrop([zh], { x: 25, y: 0, w: 500, h: 500 })), ["好世界"]);
  const mixed = line([word("價", 10, 100, 20), word("格", 30, 100, 20), word("HK$5", 50), word("only", 100)]);
  assert.deepEqual(texts(linesInCrop([mixed], { x: 25, y: 0, w: 500, h: 500 })), ["格HK$5 only"]);
  assert.equal(joinWords(["a", "", "b", "𠀀", "c"]), "a b𠀀c");
});

test("lines without word boxes use the line's centre", () => {
  const l: OcrLine = { text: "no words", x: 10, y: 100, w: 100, h: 20 };
  assert.deepEqual(texts(linesInCrop([l], { x: 0, y: 0, w: 70, h: 500 })), ["no words"]); // centre 60
  assert.deepEqual(texts(linesInCrop([l], { x: 0, y: 0, w: 50, h: 500 })), []);
});

test("boxes of a file saved after a crop are shifted by the saved crop's corner", () => {
  // The file on disk is the earlier crop starting at (200, 300); its boxes start at 0.
  const l = line([word("left", 0, 0), word("right", 300, 0)]);
  const origin = { x: 200, y: 300 };
  // In image pixels "left" spans 200..240 and "right" 500..540; the new crop is 150..400.
  const out = linesInCrop([l], { x: 150, y: 250, w: 250, h: 200 }, origin);
  assert.deepEqual(texts(out), ["left"]);
  assert.equal(out[0].x, 200);
  assert.equal(out[0].y, 300);
});

test("cropped lines still join into paragraphs", () => {
  const a = line([word("Windows", 10, 100, 60), word("doesn't", 80, 100, 60), word("hit", 150, 100, 30)]);
  const b = line([word("the", 10, 122, 30), word("error.", 50, 122, 50)]);
  const out = paragraphs(linesInCrop([a, b], { x: 0, y: 0, w: 1000, h: 1000 }));
  assert.deepEqual(out, ["Windows doesn't hit the error."]);
  const cut = paragraphs(linesInCrop([a, b], { x: 0, y: 0, w: 1000, h: 135 })); // b centre 132 in
  assert.deepEqual(cut, ["Windows doesn't hit the error."]);
  assert.deepEqual(paragraphs(linesInCrop([a, b], { x: 0, y: 0, w: 1000, h: 130 })), ["Windows doesn't hit"]);
});
