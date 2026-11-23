// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import { Latest, LABELS, SOURCES, TARGETS, debounce, pairFor, pickSource, pickTarget, pickVoice, resolve, swap } from "./logic.ts";

const SHIPPED = ["zh-Hant-en", "zh-Hans-en", "en-zh-Hant"];

test("only shipped pairs exist", () => {
  assert.equal(pairFor("en", "zh-Hant"), "en-zh-Hant");
  assert.equal(pairFor("zh-Hant", "en"), "zh-Hant-en");
  assert.equal(pairFor("zh-Hans", "en"), "zh-Hans-en");
  assert.equal(pairFor("en", "en"), null);
  assert.equal(pairFor("zh-Hant", "zh-Hant"), null);
  assert.equal(pairFor("zh-Hans", "zh-Hant"), null);
  // 简中 is never a target.
  assert.deepEqual(TARGETS, ["en", "zh-Hant"]);
  assert.deepEqual(SOURCES.map((s) => LABELS[s]), ["Auto", "English", "繁中", "简中"]);
});

test("auto follows the detected text", () => {
  assert.equal(resolve({ source: "auto", target: "en" }, "en-zh-Hant"), "en-zh-Hant");
  assert.equal(resolve({ source: "auto", target: "zh-Hant" }, "zh-Hant-en"), "zh-Hant-en");
  assert.equal(resolve({ source: "auto", target: "en" }, "zh-Hans-en"), "zh-Hans-en");
  assert.equal(resolve({ source: "auto", target: "en" }, null), null);
});

test("a fixed source uses its pair", () => {
  assert.equal(resolve({ source: "zh-Hans", target: "en" }, "en-zh-Hant"), "zh-Hans-en");
  assert.equal(resolve({ source: "en", target: "zh-Hant" }, null), "en-zh-Hant");
});

test("every pick leads to a shipped pair", () => {
  for (const source of SOURCES) {
    for (const target of TARGETS) {
      for (const pick of [...SOURCES.map((s) => pickSource({ source, target }, s)), ...TARGETS.map((t) => pickTarget({ source, target }, t))]) {
        if (pick.source === "auto") continue;
        assert.ok(SHIPPED.includes(pairFor(pick.source, pick.target) ?? ""), JSON.stringify(pick));
      }
      for (const detected of [null, ...SHIPPED] as const) {
        const s = swap({ source, target }, detected as never);
        assert.ok(SHIPPED.includes(pairFor(s.source as never, s.target) ?? ""), JSON.stringify(s));
      }
    }
  }
});

test("source and target picks", () => {
  assert.deepEqual(pickSource({ source: "auto", target: "en" }, "en"), { source: "en", target: "zh-Hant" });
  assert.deepEqual(pickSource({ source: "en", target: "zh-Hant" }, "zh-Hans"), { source: "zh-Hans", target: "en" });
  assert.deepEqual(pickSource({ source: "en", target: "zh-Hant" }, "auto"), { source: "auto", target: "zh-Hant" });
  // Into 繁中: only from English.
  assert.deepEqual(pickTarget({ source: "zh-Hans", target: "en" }, "zh-Hant"), { source: "en", target: "zh-Hant" });
  // Into English while the source is English: Auto (Chinese text).
  assert.deepEqual(pickTarget({ source: "en", target: "zh-Hant" }, "en"), { source: "auto", target: "en" });
  assert.deepEqual(pickTarget({ source: "zh-Hant", target: "en" }, "en"), { source: "zh-Hant", target: "en" });
  // Auto into 繁中 means English text.
  assert.deepEqual(pickTarget({ source: "auto", target: "en" }, "zh-Hant"), { source: "en", target: "zh-Hant" });
  assert.deepEqual(pickTarget({ source: "auto", target: "zh-Hant" }, "en"), { source: "auto", target: "en" });
});

test("swap", () => {
  assert.deepEqual(swap({ source: "en", target: "zh-Hant" }, null), { source: "zh-Hant", target: "en" });
  assert.deepEqual(swap({ source: "zh-Hant", target: "en" }, null), { source: "en", target: "zh-Hant" });
  // English to 简中 does not exist.
  assert.deepEqual(swap({ source: "zh-Hans", target: "en" }, null), { source: "en", target: "zh-Hant" });
  // Auto: the detected pair is swapped.
  assert.deepEqual(swap({ source: "auto", target: "en" }, "en-zh-Hant"), { source: "zh-Hant", target: "en" });
  assert.deepEqual(swap({ source: "auto", target: "en" }, "zh-Hans-en"), { source: "en", target: "zh-Hant" });
});

test("read aloud: a local voice for the language, never a network one", () => {
  const voices = [
    { name: "Online HK", lang: "zh-HK", localService: false },
    { name: "Tracy", lang: "zh-HK", localService: true },
    { name: "Hanhan", lang: "zh-TW", localService: true },
    { name: "Zira", lang: "en-US", localService: true },
    { name: "Mark", lang: "en_GB", localService: true, default: true },
  ];
  assert.equal(pickVoice(voices, "zh-Hant")?.name, "Tracy");
  assert.equal(pickVoice(voices.filter((v) => v.name !== "Tracy"), "zh-Hant")?.name, "Hanhan");
  assert.equal(pickVoice(voices, "en")?.name, "Mark");
  assert.equal(pickVoice(voices.filter((v) => !v.lang.startsWith("zh") || !v.localService), "zh-Hant"), null);
  // Simplified (zh-CN) is not a voice for 繁中; "eng" is not English.
  assert.equal(pickVoice([{ lang: "zh-CN", localService: true }, { lang: "eng", localService: true }], "zh-Hant"), null);
  assert.equal(pickVoice([{ lang: "eng", localService: true }], "en"), null);
  assert.equal(pickVoice([], "en"), null);
});

/** Fake timers: run() fires everything due by `now`. */
function fakeTimers() {
  let now = 0;
  let seq = 0;
  const due = new Map<number, { at: number; f: () => void }>();
  return {
    timers: {
      set: (f: () => void, ms: number) => (due.set(++seq, { at: now + ms, f }), seq),
      clear: (id: unknown) => void due.delete(id as number),
    },
    advance(ms: number) {
      now += ms;
      for (const [id, t] of [...due].sort((a, b) => a[1].at - b[1].at)) {
        if (t.at <= now) {
          due.delete(id);
          t.f();
        }
      }
    },
  };
}

test("debounce: one run after typing pauses, with the last text", () => {
  const clock = fakeTimers();
  const runs: string[] = [];
  const d = debounce((t: string) => runs.push(t), 500, clock.timers);
  d.call("H");
  clock.advance(200);
  d.call("He");
  clock.advance(200);
  d.call("Hello");
  clock.advance(499);
  assert.deepEqual(runs, []);
  clock.advance(1);
  assert.deepEqual(runs, ["Hello"]);
  d.call("Hello!");
  d.cancel();
  clock.advance(1000);
  assert.deepEqual(runs, ["Hello"]);
});

test("latest wins: an older translation that finishes last is dropped", async () => {
  const latest = new Latest();
  const shown: string[] = [];
  const translate = async (text: string, ms: number) => {
    const current = latest.begin();
    await new Promise((r) => setTimeout(r, ms));
    if (current()) shown.push(text);
  };
  await Promise.all([translate("old", 30), translate("new", 5)]);
  assert.deepEqual(shown, ["new"]);
  const current = latest.begin();
  latest.cancel();
  assert.equal(current(), false);
});
