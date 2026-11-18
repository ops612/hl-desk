// npm test (node --test; Node 22.18+ runs TypeScript directly). Not part of tsc (tsconfig exclude).
import assert from "node:assert/strict";
import { test } from "node:test";
import { barScales, clock, meterState, onLevel, type Meter } from "./voice-meter.ts";

const start: Meter = { since: 1000, lastEventAt: null, lastSoundAt: null };

test("live right after the recording is confirmed", () => {
  assert.equal(meterState(start, 1000), "live");
});

test("stale when no level event comes for over 1 s", () => {
  assert.equal(meterState(start, 2000), "live");
  assert.equal(meterState(start, 2001), "stale");
  const m = onLevel(start, 0.3, 1900);
  assert.equal(meterState(m, 2900), "live");
  assert.equal(meterState(m, 2901), "stale");
});

test("silent after 3 s of events below the silence peak, live again on sound", () => {
  let m = start;
  for (let t = 1100; t <= 4000; t += 100) m = onLevel(m, 0.004, t);
  assert.equal(meterState(m, 3999), "live");
  assert.equal(meterState(m, 4000), "silent");
  m = onLevel(m, 0.2, 4100);
  assert.equal(meterState(m, 4100), "live");
  for (let t = 4200; t <= 7100; t += 100) m = onLevel(m, 0, t);
  assert.equal(meterState(m, 7100), "silent");
});

test("stale wins over silent when events stop", () => {
  const m = onLevel(start, 0, 1100);
  assert.equal(meterState(m, 5000), "stale");
});

test("bar scales stay in range and grow with the level", () => {
  const quiet = barScales(0);
  const loud = barScales(1);
  assert.equal(quiet.length, 5);
  for (const v of [...quiet, ...loud, ...barScales(7), ...barScales(-1)]) assert.ok(v >= 0.2 && v <= 1);
  assert.ok(loud[2] > quiet[2]);
  assert.deepEqual(barScales(0.3), barScales(0.3));
});

test("clock", () => {
  assert.equal(clock(0), "0:00");
  assert.equal(clock(7_900), "0:07");
  assert.equal(clock(65_000), "1:05");
});
