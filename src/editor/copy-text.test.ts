// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import { ONLY_ENGLISH, copiedLabel, countLines, isNoChineseError, joinParagraphs, penCursor } from "./copy-text.ts";
import { paragraphs } from "./ocr-paragraphs.ts";

test("label for N lines, one line and none", () => {
  assert.equal(copiedLabel(3), "Text copied · 3 lines");
  assert.equal(copiedLabel(1), "Text copied · 1 line");
  assert.equal(copiedLabel(0), "No text found");
});

test("paragraphs join as plain text, one per line", () => {
  const lines = [
    { text: "Windows doesn't hit the", x: 10, y: 10, w: 200, h: 14 },
    { text: "error now.", x: 10, y: 28, w: 80, h: 14 },
    { text: "  ", x: 10, y: 46, w: 10, h: 14 },
    { text: "Next step", x: 10, y: 80, w: 90, h: 14 },
  ];
  const text = joinParagraphs(paragraphs(lines));
  assert.equal(text, "Windows doesn't hit the error now.\nNext step");
  assert.equal(countLines(text), 2);
  assert.equal(joinParagraphs([" a ", "", "b"]), "a\nb");
  assert.equal(joinParagraphs([]), "");
  assert.equal(countLines(""), 0);
});

test("the no-Chinese OCR error is recognised", () => {
  assert.ok(isNoChineseError("ocr-no-chinese: Chinese text recognition isn't installed on this PC. Installed: en-US."));
  assert.ok(!isNoChineseError("Could not read the screenshot: x"));
  assert.match(ONLY_ENGLISH, /^Only English can be read\./);
});

test("pen cursor is a dot in each colour with a centre hotspot", () => {
  for (const color of ["#2F4CC8", "#E5484D", "#0B1220"]) {
    const css = penCursor(color, 3);
    assert.ok(css.startsWith('url("data:image/svg+xml,'));
    assert.ok(css.endsWith('") 4 4, crosshair'), css); // 6 px dot + 1 px outline: 8 px box
    const svg = decodeURIComponent(css.slice(css.indexOf(",") + 1, css.indexOf('")')));
    assert.ok(svg.includes(`fill="${color}"`), svg);
    assert.ok(svg.includes('stroke="#fff"'));
    assert.ok(svg.includes('width="8" height="8"'));
  }
  // Larger strokes give a larger dot, still centred.
  assert.ok(penCursor("#2F4CC8", 10).endsWith('") 6 6, crosshair'));
});
