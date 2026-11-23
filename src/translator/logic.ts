// Translate window (translator.html): the pure parts, tested by logic.test.ts (npm test).
// Language pickers: only the pairs the app ships (src-tauri/src/translate.rs Pair) can be chosen,
// so every source and target choice is turned into one of them. Plus a debounce and a
// latest-wins guard so an older translation never overwrites a newer one.
import type { PairId } from "../translate/engine";

export type Lang = "en" | "zh-Hant" | "zh-Hans";
export type Source = "auto" | Lang;
export type Target = "en" | "zh-Hant";

export const SOURCES: Source[] = ["auto", "en", "zh-Hant", "zh-Hans"];
export const TARGETS: Target[] = ["en", "zh-Hant"];
export const LABELS: Record<Source, string> = { auto: "Auto", en: "English", "zh-Hant": "繁中", "zh-Hans": "简中" };

/** The shipped pair from `from` to `to`, or null (e.g. English to 简中). */
export function pairFor(from: Lang, to: Target): PairId | null {
  if (from === "en" && to === "zh-Hant") return "en-zh-Hant";
  if (from === "zh-Hant" && to === "en") return "zh-Hant-en";
  if (from === "zh-Hans" && to === "en") return "zh-Hans-en";
  return null;
}

/** The language a pair translates from (the "Detected" chip). */
export function sourceOf(pair: PairId): Lang {
  return pair === "en-zh-Hant" ? "en" : pair === "zh-Hans-en" ? "zh-Hans" : "zh-Hant";
}

/** The language a pair translates into. */
export function targetOf(pair: PairId): Target {
  return pair === "en-zh-Hant" ? "zh-Hant" : "en";
}

/** The one target a fixed source can go to: Chinese to English, English to 繁中. */
function targetFor(from: Lang): Target {
  return from === "en" ? "zh-Hant" : "en";
}

export interface Picks {
  source: Source;
  target: Target;
}

/** The pair to use. Auto follows the detected pair (Chinese to English, English to 繁中); none
 * detected yet (empty text, no letters): null. */
export function resolve(picks: Picks, detected: PairId | null): PairId | null {
  if (picks.source === "auto") return detected;
  return pairFor(picks.source, picks.target);
}

/** A new source: the target follows so the pair is one we ship (Auto keeps the target, which
 * then follows the detected text). */
export function pickSource(picks: Picks, source: Source): Picks {
  if (source === "auto") return { source, target: picks.target };
  return { source, target: targetFor(source) };
}

/** A new target: a source that cannot go there changes (into 繁中 only from English; into
 * English from Chinese, Auto when it was English or Auto). */
export function pickTarget(picks: Picks, target: Target): Picks {
  if (picks.source !== "auto" && pairFor(picks.source, target)) return { source: picks.source, target };
  return { source: target === "zh-Hant" ? "en" : "auto", target };
}

/** Swap: the current target becomes the source (`detected` stands in for Auto) and the target
 * follows it. English to 简中 does not exist, so 简中 to English swaps to English to 繁中. */
export function swap(picks: Picks, detected: PairId | null): Picks {
  const to = picks.source === "auto" && detected ? targetOf(detected) : picks.target;
  return { source: to, target: targetFor(to) };
}

// ---------- read aloud ----------

export interface VoiceLike {
  lang: string;
  localService: boolean;
  default?: boolean;
}

/** A voice on this computer (never a network one) for `lang`: English, or for 繁中 Cantonese
 * (zh-HK) first, then Taiwan (zh-TW). None: the Read aloud icon is hidden. */
export function pickVoice<V extends VoiceLike>(voices: readonly V[], lang: Target): V | null {
  const local = voices.filter((v) => v.localService);
  const norm = (v: V) => v.lang.replace("_", "-").toLowerCase();
  const tags = lang === "en" ? ["en"] : ["zh-hk", "yue", "zh-tw", "zh-hant"];
  for (const t of tags) {
    const found = local.filter((v) => norm(v) === t || norm(v).startsWith(`${t}-`));
    if (found.length) return found.find((v) => v.default) ?? found[0];
  }
  return null;
}

// ---------- typing ----------

/** Runs `fn` once typing pauses for `ms` (each call restarts the wait). */
export function debounce<A extends unknown[]>(
  fn: (...args: A) => void,
  ms: number,
  timers: { set: (f: () => void, ms: number) => unknown; clear: (id: unknown) => void } = {
    set: (f, ms) => setTimeout(f, ms),
    clear: (id) => clearTimeout(id as ReturnType<typeof setTimeout>),
  },
): { call(...args: A): void; cancel(): void } {
  let id: unknown = null;
  return {
    call(...args: A) {
      if (id !== null) timers.clear(id);
      id = timers.set(() => {
        id = null;
        fn(...args);
      }, ms);
    },
    cancel() {
      if (id !== null) timers.clear(id);
      id = null;
    },
  };
}

/** Latest wins: `begin()` starts a run and returns its check; any later `begin()` or `cancel()`
 * makes earlier checks false, so a slow older translation is dropped. */
export class Latest {
  private n = 0;
  begin(): () => boolean {
    const mine = ++this.n;
    return () => mine === this.n;
  }
  cancel(): void {
    this.n++;
  }
}
