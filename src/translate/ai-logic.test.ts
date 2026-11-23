// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import { AI_MAX_CHARS, AI_OFFLINE, AI_TOO_LONG, AiModel, type AiTarget, type AiView, charCount } from "./ai-logic.ts";

/** A model whose calls wait until the test settles them. */
function setup() {
  const views: AiView[] = [];
  const calls: { text: string; target: AiTarget; resolve: (s: string) => void; reject: (e: unknown) => void }[] = [];
  const model = new AiModel(
    (text, target) => new Promise<string>((resolve, reject) => calls.push({ text, target, resolve, reject })),
    (v) => views.push(v),
  );
  model.available = true;
  return { model, views, calls, last: () => views[views.length - 1] };
}

test("hidden when off, not built in, or no text; no request", async () => {
  const { model, calls, last } = setup();
  model.available = false;
  await model.run("hello", "zh-Hant");
  assert.deepEqual(last(), { kind: "hidden" });
  model.available = true;
  await model.run("   ", "zh-Hant");
  assert.deepEqual(last(), { kind: "hidden" });
  await model.run("hello", null);
  assert.deepEqual(last(), { kind: "hidden" });
  assert.equal(calls.length, 0);
});

test("pending, then the translation", async () => {
  const { model, calls, views, last } = setup();
  const p = model.run("你好", "en");
  assert.deepEqual(last(), { kind: "pending", text: "Translating…" });
  assert.deepEqual(calls.map((c) => [c.text, c.target]), [["你好", "en"]]);
  calls[0].resolve("Hello");
  await p;
  assert.deepEqual(last(), { kind: "done", text: "Hello" });
  // The next input keeps the last result muted while it waits.
  void model.run("你好嗎", "en");
  assert.deepEqual(last(), { kind: "pending", text: "Hello" });
  assert.equal(views.length, 3);
});

test("latest wins: an older reply never overwrites a newer one", async () => {
  const { model, calls, last } = setup();
  const first = model.run("one", "zh-Hant");
  const second = model.run("two", "zh-Hant");
  calls[1].resolve("二");
  await second;
  calls[0].resolve("一");
  await first;
  assert.deepEqual(last(), { kind: "done", text: "二" });
  // An older failure is dropped too.
  const third = model.run("three", "zh-Hant");
  const fourth = model.run("four", "zh-Hant");
  calls[3].resolve("四");
  await fourth;
  calls[2].reject("AI is offline right now");
  await third;
  assert.deepEqual(last(), { kind: "done", text: "四" });
});

test("cancel drops the running request and hides the panel", async () => {
  const { model, calls, last } = setup();
  const p = model.run("bye", "zh-Hant");
  model.cancel();
  assert.deepEqual(last(), { kind: "hidden" });
  calls[0].resolve("再見");
  await p;
  assert.deepEqual(last(), { kind: "hidden" });
});

test("too long for AI: no request; the limit counts characters, not UTF-16 units", async () => {
  const { model, calls, last } = setup();
  await model.run("a".repeat(AI_MAX_CHARS + 1), "zh-Hant");
  assert.deepEqual(last(), { kind: "error", message: AI_TOO_LONG });
  assert.equal(calls.length, 0);
  // 2000 emoji are 4000 UTF-16 units but 2000 characters: still sent.
  const emoji = "😀".repeat(AI_MAX_CHARS);
  assert.equal(charCount(emoji), AI_MAX_CHARS);
  void model.run(emoji, "en");
  assert.equal(calls.length, 1);
});

test("errors show the message from Rust, or the offline line", async () => {
  const { model, calls, last } = setup();
  const a = model.run("x", "en");
  calls[0].reject("AI limit reached for this month");
  await a;
  assert.deepEqual(last(), { kind: "error", message: "AI limit reached for this month" });
  const b = model.run("y", "en");
  calls[1].reject(new Error("ipc failed"));
  await b;
  assert.deepEqual(last(), { kind: "error", message: AI_OFFLINE });
});

test("the same input again shows the last result without a new request", async () => {
  const { model, calls, last } = setup();
  const p = model.run("hi", "zh-Hant");
  calls[0].resolve("你好");
  await p;
  await model.run("hi", "zh-Hant");
  assert.deepEqual(last(), { kind: "done", text: "你好" });
  assert.equal(calls.length, 1);
  // Another target is a new request.
  void model.run("hi", "zh-Hans");
  assert.equal(calls.length, 2);
});
