// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import { toastSetup } from "./kind.ts";

test("copied: the capture id and about 4 s", () => {
  assert.deepEqual(toastSetup("?kind=copied&id=42"), { kind: "copied", id: 42, closeAfterMs: 4000 });
});

test("copied without a usable id has none (no editor-NaN)", () => {
  assert.equal(toastSetup("?kind=copied").id, null);
  assert.equal(toastSetup("?kind=copied&id=abc").id, null);
  assert.equal(toastSetup("?kind=copied&id=0").id, null);
});

test("existing kinds keep their timings and carry no id", () => {
  assert.deepEqual(toastSetup("?kind=cancelled"), { kind: "cancelled", id: null, closeAfterMs: 2000 });
  assert.deepEqual(toastSetup("?kind=shot&id=5"), { kind: "shot", id: null, closeAfterMs: 8000 });
});
