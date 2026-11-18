// What the note editor's voice line shows during an offline recording, from the microphone
// level events (voice.rs, "voice://level", about every 100 ms). Pure, so it is unit tested
// (voice-meter.test.ts, npm test): the line must never say "Listening" when nothing is heard.

/** Below this peak (0..1) a level event counts as silence. */
export const SILENCE_PEAK = 0.01;
/** Silence this long: "No sound from the microphone". */
export const SILENCE_MS = 3000;
/** No level event this long: the bars lie flat (the meter is stale, not the room silent). */
export const STALE_MS = 1000;

/** "live": bars follow the level; "silent": no sound for SILENCE_MS; "stale": no events for STALE_MS. */
export type MeterState = "live" | "silent" | "stale";

export interface Meter {
  /** When Rust confirmed the recording (ms). */
  since: number;
  /** The last level event (ms), or null before the first. */
  lastEventAt: number | null;
  /** The last event at or above SILENCE_PEAK (ms), or null before the first. */
  lastSoundAt: number | null;
}

export function meterState(m: Meter, now: number): MeterState {
  if (now - (m.lastEventAt ?? m.since) > STALE_MS) return "stale";
  if (now - (m.lastSoundAt ?? m.since) >= SILENCE_MS) return "silent";
  return "live";
}

/** The meter after a level event at `now`. */
export function onLevel(m: Meter, level: number, now: number): Meter {
  return { ...m, lastEventAt: now, lastSoundAt: level >= SILENCE_PEAK ? now : m.lastSoundAt };
}

/** Fixed per-bar weights, so five bars from one level read as a voice, not a block. */
const WEIGHTS = [0.55, 0.85, 1, 0.75, 0.5];
/** Lowest bar scale, so a quiet room still shows five small bars. */
const FLOOR = 0.2;

/** Bar scales (FLOOR..1) for a peak level 0..1. A square root lifts speech, which peaks low. */
export function barScales(level: number): number[] {
  const v = Math.sqrt(Math.min(1, Math.max(0, level)));
  return WEIGHTS.map((w) => FLOOR + (1 - FLOOR) * Math.min(1, v * w * 1.6));
}

/** "0:07", "1:05": elapsed time for the line. */
export function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`;
}
