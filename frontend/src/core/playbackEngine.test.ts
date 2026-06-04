import { describe, expect, it } from "vitest";

import { shouldApplyRemoteSeek } from "./playbackEngine";

/**
 * Guards the remote-seek gate that decides whether an incoming server
 * position should snap the locally-playing element. The bug this protects
 * against: an older TV (the audio output) restarting the current song every
 * time the DM changed the volume or edited the queue, because the broadcast
 * carried a stale `position_ms` that the old logic mistook for a seek.
 */

const THRESHOLD = 1500;
const seek = (
  targetMs: number,
  lastReportedMs: number,
  elapsedMs: number,
): boolean =>
  shouldApplyRemoteSeek({ targetMs, lastReportedMs, elapsedMs, thresholdMs: THRESHOLD });

describe("shouldApplyRemoteSeek", () => {
  it("ignores an unrelated broadcast that echoes our last report", () => {
    // DM changed the volume 30s into the track. The server didn't touch
    // position_ms, so it echoes back the 30s we last reported — not a seek.
    expect(seek(30_000, 30_000, 30_400)).toBe(false);
  });

  it("ignores a stale echo even when reports have lagged far behind", () => {
    // The core "older TV" failure: reports stalled at the track start, so the
    // server still has position_ms≈0 and echoes it while the element is 30s
    // in. Comparing against the element time would yank it back to 0; comparing
    // against our own (equally stale) telemetry correctly does nothing.
    expect(seek(0, 0, 30_000)).toBe(false);
  });

  it("snaps when the server position diverges from our telemetry (real seek)", () => {
    // DM dragged the scrub bar to 60s while we were reporting ~5s.
    expect(seek(60_000, 5_000, 5_200)).toBe(true);
  });

  it("snaps a loop:track restart back to zero", () => {
    // Track ended, server reset position_ms to 0 on the same track id while
    // the element is still sitting at the very end.
    expect(seek(0, 178_000, 179_000)).toBe(true);
  });

  it("does not re-seek when the element already sits at the target", () => {
    // Server position diverged from telemetry, but the element is already
    // there (a redundant broadcast after a seek already applied).
    expect(seek(60_000, 5_000, 60_000)).toBe(false);
  });

  it("ignores sub-threshold nudges", () => {
    expect(seek(2_000, 1_000, 2_000)).toBe(false);
  });

  it("never seeks on a non-finite element time", () => {
    expect(seek(60_000, 0, Number.NaN)).toBe(false);
  });
});
