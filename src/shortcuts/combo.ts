// Global shortcut combos (Settings > Shortcuts). Pure functions, tested by combo.test.ts.
// A combo is stored as modifiers then one key, joined by "+": "Ctrl+Shift+Digit2",
// "Shift+Super+Space". Modifiers are Ctrl, Alt, Shift, Super (Cmd on macOS, Win on Windows),
// always in that order; the key is a KeyboardEvent.code. Rust parses the same strings
// (src-tauri/src/shortcuts.rs).

export type ShortcutId = "open" | "capture" | "translate";
export const SHORTCUT_IDS: ShortcutId[] = ["open", "capture", "translate"];
export type Combos = Record<ShortcutId, string>;

const MODS = ["Ctrl", "Alt", "Shift", "Super"] as const;
type Mod = (typeof MODS)[number];

/** The built-in combos: Cmd on macOS, Ctrl elsewhere (same as before 0.1.14). */
export function defaults(mac: boolean): Combos {
  const main = mac ? "Shift+Super" : "Ctrl+Shift";
  return { open: `${main}+Space`, capture: `${main}+Digit2`, translate: `${main}+Digit1` };
}

/** Keys a shortcut may end with: letters, digits, F1-F12, Space and the punctuation keys. */
const PUNCT: Record<string, string> = {
  Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";",
  Quote: "'", Comma: ",", Period: ".", Slash: "/", Backquote: "`",
};
function keyLabel(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F([1-9]|1[0-2])$/.test(code)) return code;
  if (code === "Space") return "Space";
  return PUNCT[code] ?? null;
}

type KeyEventLike = { code: string; ctrlKey: boolean; altKey: boolean; shiftKey: boolean; metaKey: boolean };

/** The combo a key press makes, or null to keep waiting (a modifier alone, a bare key, or a key
 * that cannot be a shortcut). Esc is handled by the caller before this. */
export function comboFromEvent(e: KeyEventLike): string | null {
  if (keyLabel(e.code) === null) return null;
  const mods: Mod[] = [];
  if (e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  if (e.metaKey) mods.push("Super");
  if (mods.length === 0) return null;
  return [...mods, e.code].join("+");
}

/** Splits a stored combo; null when it is not a valid one. */
export function parseCombo(combo: string): { mods: Mod[]; code: string } | null {
  const parts = combo.split("+");
  const code = parts.pop() ?? "";
  if (keyLabel(code) === null) return null;
  const mods = MODS.filter((m) => parts.includes(m));
  if (mods.length === 0 || mods.length !== parts.length) return null;
  return { mods, code };
}

/** One canonical spelling, so two spellings of the same combo compare equal. */
export function canonical(combo: string): string {
  const p = parseCombo(combo);
  return p ? [...p.mods, p.code].join("+") : combo;
}

const MAC_SYMBOL: Record<Mod, string> = { Super: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧" };
const MAC_WORD: Record<Mod, string> = { Super: "Cmd", Ctrl: "Ctrl", Alt: "Option", Shift: "Shift" };
const WIN_WORD: Record<Mod, string> = { Ctrl: "Ctrl", Alt: "Alt", Shift: "Shift", Super: "Win" };
/** macOS shows Cmd first, as the app always has (⌘⇧2). */
const MAC_ORDER: Mod[] = ["Super", "Ctrl", "Alt", "Shift"];

/** The combo as keycaps: ["Ctrl", "Shift", "2"] on Windows, ["⌘", "⇧", "2"] on macOS
 * (words: ["Cmd", "Shift", "2"]). */
export function comboParts(combo: string, mac: boolean, words = false): string[] {
  const p = parseCombo(combo);
  if (!p) return [combo];
  const mods = mac ? MAC_ORDER.filter((m) => p.mods.includes(m)) : p.mods;
  const names = mac ? (words ? MAC_WORD : MAC_SYMBOL) : WIN_WORD;
  return [...mods.map((m) => names[m]), keyLabel(p.code)!];
}

/** The combo as text: "Ctrl+Alt+T" on Windows, "⌘⇧1" on macOS ("Cmd+Shift+1" with words). */
export function formatCombo(combo: string, mac: boolean, words = false): string {
  const parts = comboParts(combo, mac, words);
  return mac && !words ? parts.join("") : parts.join("+");
}

/** A changed chip is tinted. */
export function isChanged(id: ShortcutId, combos: Combos, mac: boolean): boolean {
  return canonical(combos[id]) !== canonical(defaults(mac)[id]);
}

/** "Reset all to defaults" shows only when something differs. */
export function anyChanged(combos: Combos, mac: boolean): boolean {
  return SHORTCUT_IDS.some((id) => isChanged(id, combos, mac));
}

export const LABELS: Record<ShortcutId, string> = {
  open: "Open or close HL Desk",
  capture: "Capture an area",
  translate: "Translate window",
};

/** Why a change was refused (Rust shortcuts_set): another app holds the combo, or another
 * HL Desk shortcut ("open", "capture", "translate", or "device": the capture key setting). */
export type Refusal = { id: string; combo: string; kept: string; taken: boolean; other: string | null };

/** The red line under the row. */
export function refusalText(r: Refusal, mac: boolean): string {
  const combo = formatCombo(r.combo, mac);
  const kept = formatCombo(r.kept, mac);
  if (r.taken || !r.other) return `${combo} is in use by another app. Kept ${kept}.`;
  const by = r.other === "device" ? "the Capture key setting" : LABELS[r.other as ShortcutId] ?? r.other;
  return `${combo} is used by ${by}. Kept ${kept}.`;
}
