import { describe, expect, it } from "vitest";

import {
  EQ_FREQUENCIES,
  EQ_GAIN_MAX,
  EQ_GAIN_MIN,
  clampGain,
  computeEqResponseDb,
  defaultEqBands,
  isFlat,
  logFreqAxis,
  normalizeEqBands,
} from "./eq";

describe("eq math", () => {
  it("a flat band set has ~0 dB response everywhere", () => {
    const resp = computeEqResponseDb(defaultEqBands(), [32, 250, 1000, 8000]);
    for (const db of resp) expect(Math.abs(db)).toBeLessThan(1e-6);
  });

  it("a single +12 dB band peaks near its centre and is ~flat far away", () => {
    const bands = defaultEqBands().map((b) =>
      b.frequency === 1000 ? { ...b, gain: 12 } : b,
    );
    const [atCentre, atLow, atHigh] = computeEqResponseDb(bands, [1000, 32, 16000]);
    // Peak sits close to the full boost at the centre frequency…
    expect(atCentre).toBeGreaterThan(10);
    expect(atCentre).toBeLessThanOrEqual(12.5);
    // …and a couple of octaves away the one-octave band has fallen off.
    expect(Math.abs(atLow)).toBeLessThan(1);
    expect(Math.abs(atHigh)).toBeLessThan(1);
  });

  it("a cut produces negative dB at the centre", () => {
    const bands = defaultEqBands().map((b) =>
      b.frequency === 250 ? { ...b, gain: -9 } : b,
    );
    const [atCentre] = computeEqResponseDb(bands, [250]);
    expect(atCentre).toBeLessThan(-7);
  });

  it("normalizeEqBands always yields the canonical band set, flat by default", () => {
    const bands = normalizeEqBands(undefined);
    expect(bands).toHaveLength(EQ_FREQUENCIES.length);
    expect(bands.map((b) => b.frequency)).toEqual([...EQ_FREQUENCIES]);
    expect(isFlat(bands)).toBe(true);
  });

  it("normalizeEqBands keeps provided gains (by index) and clamps out-of-range", () => {
    const raw = [{ gain: 3 }, { gain: 99 }, { gain: -99 }];
    const bands = normalizeEqBands(raw);
    expect(bands[0].gain).toBe(3);
    expect(bands[1].gain).toBe(EQ_GAIN_MAX);
    expect(bands[2].gain).toBe(EQ_GAIN_MIN);
    expect(bands[3].gain).toBe(0); // missing → flat
  });

  it("clampGain handles non-finite input", () => {
    expect(clampGain(Number.NaN)).toBe(0);
    expect(clampGain(50)).toBe(EQ_GAIN_MAX);
  });

  it("logFreqAxis spans the range in log steps", () => {
    const axis = logFreqAxis(3, 20, 20000);
    expect(axis[0]).toBeCloseTo(20, 5);
    expect(axis[2]).toBeCloseTo(20000, 5);
    expect(axis[1]).toBeCloseTo(632.45, 1); // geometric midpoint
  });
});
