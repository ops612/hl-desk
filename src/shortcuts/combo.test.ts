// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import {
  anyChanged, canonical, comboFromEvent, comboParts, defaults, formatCombo, isChanged, parseCombo, refusalText,
} from "./combo.ts";

const key = (code: string, mods: { ctrl?: boolean; alt?: boolean; shift?: boolean; meta?: boolean } = {}) => ({
  code, ctrlKey: !!mods.ctrl, altKey: !!mods.alt, shiftKey: !!mods.shift, metaKey: !!mods.meta,
});

test("a combo needs a modifier and a normal key", () => {
  assert.equal(comboFromEvent(key("KeyT", { ctrl: true, alt: true })), "Ctrl+Alt+KeyT");
  assert.equal(comboFromEvent(key("Digit1", { meta: true, shift: true })), "Shift+Super+Digit1");
  assert.equal(comboFromEvent(key("F5", { alt: true })), "Alt+F5");
  assert.equal(comboFromEvent(key("Space", { ctrl: true, shift: true })), "Ctrl+Shift+Space");
});

test("bare keys, modifiers alone and unsupported keys keep waiting", () => {
  assert.equal(comboFromEvent(key("KeyT")), null);
  assert.equal(comboFromEvent(key("Digit2")), null);
  assert.equal(comboFromEvent(key("ShiftLeft", { shift: true })), null);
  assert.equal(comboFromEvent(key("ControlLeft", { ctrl: true })), null);
  assert.equal(comboFromEvent(key("Enter", { ctrl: true })), null);
  assert.equal(comboFromEvent(key("ArrowUp", { ctrl: true })), null);
});

test("parse rejects combos without a modifier or with an unknown part", () => {
  assert.equal(parseCombo("KeyT"), null);
  assert.equal(parseCombo("Ctrl+Hyper+KeyT"), null);
  assert.equal(parseCombo("Ctrl+Enter"), null);
  assert.deepEqual(parseCombo("Shift+Ctrl+KeyT"), { mods: ["Ctrl", "Shift"], code: "KeyT" });
  assert.equal(canonical("Shift+Ctrl+KeyT"), "Ctrl+Shift+KeyT");
});

test("Windows shows words joined by +", () => {
  assert.equal(formatCombo("Ctrl+Alt+KeyT", false), "Ctrl+Alt+T");
  assert.equal(formatCombo("Ctrl+Shift+Digit2", false), "Ctrl+Shift+2");
  assert.equal(formatCombo("Ctrl+Super+Slash", false), "Ctrl+Win+/");
  assert.deepEqual(comboParts("Ctrl+Shift+Space", false), ["Ctrl", "Shift", "Space"]);
});

test("macOS shows symbols with Cmd first, or words", () => {
  assert.equal(formatCombo("Shift+Super+Digit1", true), "⌘⇧1");
  assert.equal(formatCombo("Ctrl+Alt+Shift+Super+KeyK", true), "⌘⌃⌥⇧K");
  assert.equal(formatCombo("Shift+Super+Digit1", true, true), "Cmd+Shift+1");
  assert.equal(formatCombo("Alt+KeyT", true, true), "Option+T");
  assert.deepEqual(comboParts("Shift+Super+Space", true), ["⌘", "⇧", "Space"]);
});

test("defaults are the keys HL Desk always had", () => {
  assert.equal(formatCombo(defaults(false).open, false), "Ctrl+Shift+Space");
  assert.equal(formatCombo(defaults(false).capture, false), "Ctrl+Shift+2");
  assert.equal(formatCombo(defaults(false).translate, false), "Ctrl+Shift+1");
  assert.equal(formatCombo(defaults(true).capture, true), "⌘⇧2");
});

test("changed chips and the reset link", () => {
  const win = defaults(false);
  assert.equal(anyChanged(win, false), false);
  assert.equal(isChanged("open", win, false), false);
  const changed = { ...win, capture: "Ctrl+Alt+KeyS" };
  assert.equal(isChanged("capture", changed, false), true);
  assert.equal(isChanged("open", changed, false), false);
  assert.equal(anyChanged(changed, false), true);
  // Same combo spelled in another order still counts as the default.
  assert.equal(anyChanged({ ...win, open: "Shift+Ctrl+Space" }, false), false);
  // The Windows defaults are changed keys on a Mac.
  assert.equal(anyChanged(win, true), true);
});

test("refusal lines", () => {
  assert.equal(
    refusalText({ id: "capture", combo: "Ctrl+Alt+KeyT", kept: "Ctrl+Shift+Digit2", taken: true, other: null }, false),
    "Ctrl+Alt+T is in use by another app. Kept Ctrl+Shift+2.",
  );
  assert.equal(
    refusalText({ id: "capture", combo: "Ctrl+Shift+Digit1", kept: "Ctrl+Shift+Digit2", taken: false, other: "translate" }, false),
    "Ctrl+Shift+1 is used by Translate window. Kept Ctrl+Shift+2.",
  );
  assert.equal(
    refusalText({ id: "open", combo: "Shift+Super+Digit4", kept: "Shift+Super+Space", taken: false, other: "device" }, true),
    "⌘⇧4 is used by the Capture key setting. Kept ⌘⇧Space.",
  );
});
