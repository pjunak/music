import { describe, expect, it } from "vitest";

import { shouldApplyEpochSeek } from "./playbackEngine";

/**
 * Guards the epoch seek gate. Seeks are protocol events now: the server bumps
 * `position_epoch` on every deliberate move and broadcasts materialized
 * (always-current) positions. The bug class this protects against: outputs
 * restarting or yanking the current song on volume changes / queue edits /
 * device joins, because those broadcasts carried positions the old heuristics
 * mistook for seeks.
 */

const EPSILON = 300;
const seek = (
  prevEpoch: number | null,
  epoch: number,
  elapsedMs: number,
  targetMs: number,
): boolean =>
  shouldApplyEpochSeek({ prevEpoch, epoch, elapsedMs, targetMs, epsilonMs: EPSILON });

describe("shouldApplyEpochSeek", () => {
  it("never seeks when the epoch is unchanged — whatever the positions say", () => {
    // Volume change 30s in: same epoch, positions agree.
    expect(seek(4, 4, 30_400, 30_450)).toBe(false);
    // Same epoch but positions wildly apart (a lagging TV element): still no
    // seek — drift correction is the report channel's job, not a yank.
    expect(seek(4, 4, 30_000, 0)).toBe(false);
    expect(seek(4, 4, 1_000, 120_000)).toBe(false);
  });

  it("seeks when the epoch changed and the element is elsewhere", () => {
    // Operator dragged the scrub bar to 60s.
    expect(seek(4, 5, 5_200, 60_000)).toBe(true);
    // loop:track restart back to 0 while the element sits at the very end.
    expect(seek(4, 5, 179_000, 0)).toBe(true);
  });

  it("suppresses the seek when the element already sits at the target", () => {
    // A duck-interrupt ended: ambient kept playing and the server clock kept
    // ticking alongside — the epoch bumped but there's nothing to correct.
    expect(seek(4, 5, 60_050, 60_000)).toBe(false);
  });

  it("does not treat the first observed state as a seek", () => {
    // Fresh engine / just claimed the output role: the track-change path
    // positions the element; the epoch gate stays quiet.
    expect(seek(null, 7, 0, 45_000)).toBe(false);
  });

  it("seeks on epoch change even when the element time is non-finite", () => {
    // Element still loading (NaN currentTime): the epsilon dead-band can't
    // apply, and the seek must land once metadata is there.
    expect(seek(4, 5, Number.NaN, 60_000)).toBe(true);
  });
});
