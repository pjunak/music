import assert from "node:assert/strict";
import test from "node:test";

import { compareRecords, parseOptions, renderReport } from "./voice-differential.mjs";

const checksum = "b734bca3fc99257cf0088211b44bd36e8a26fbb1f9ce67e1e97d39f188094b0a";

function record(score, coverage, windows = 10) {
  return {
    schema_version: "voice-probe/v1",
    index: 0,
    status: "classified",
    voice_score: score,
    vocal_coverage: coverage,
    prediction_windows: windows,
    model_sha256: checksum,
    elapsed_seconds: 0.5,
  };
}

test("comparison accepts bounded numeric drift with the same evidence bucket", () => {
  const comparison = compareRecords(
    [record(0.7, 0.8)],
    [record(0.68, 0.75)],
    { scoreTolerance: 0.05, coverageTolerance: 0.1, identities: ["opaque001"] },
  );
  assert.deepEqual(comparison.failures, []);
  assert.equal(comparison.rows[0].passed, true);
});

test("comparison rejects threshold, window, and semantic-bucket drift", () => {
  const comparison = compareRecords(
    [record(0.65, 0.6, 10)],
    [record(0.54, 0.19, 9)],
    { scoreTolerance: 0.05, coverageTolerance: 0.1, identities: ["opaque002"] },
  );
  assert.equal(comparison.rows[0].passed, false);
  assert.equal(comparison.failures.length, 4);
});

test("comparison does not coerce missing fields into valid zeroes", () => {
  const malformed = {
    ...record(0.2, 0.1),
    voice_score: null,
    model_sha256: "wrong",
    prediction_windows: null,
    elapsed_seconds: Number.NaN,
  };
  const comparison = compareRecords(
    [malformed],
    [record(0.2, 0.1)],
    { scoreTolerance: 0.05, coverageTolerance: 0.1, identities: ["opaque003"] },
  );
  assert.equal(comparison.rows[0].passed, false);
  assert.match(comparison.failures.join("\n"), /checksum|numeric|window|elapsed/u);
});

test("runtime options cannot weaken the checked-in quality floor", () => {
  const required = [
    "--python", "python",
    "--model", "model.pb",
    "--ffmpeg", "ffmpeg",
    "--corpus", "corpus",
  ];
  assert.throws(
    () => parseOptions([...required, "--score-tolerance", "0.051"]),
    /not exceed/u,
  );
  assert.throws(
    () => parseOptions([...required, "--coverage-tolerance", "0.101"]),
    /not exceed/u,
  );
  assert.throws(
    () => parseOptions([...required, "--minimum-tracks", "5"]),
    /from 6 through 512/u,
  );
});

test("report contains only supplied opaque identities and aggregate evidence", () => {
  const comparison = compareRecords(
    [record(0.4, 0.3)],
    [record(0.4, 0.3)],
    { scoreTolerance: 0.05, coverageTolerance: 0.1, identities: ["track-001"] },
  );
  const report = renderReport(comparison, {
    commit: "0123456789abcdef",
    skippedSymlinks: 1,
    scoreTolerance: 0.05,
    coverageTolerance: 0.1,
  });
  assert.match(report, /Status: \*\*PASS\*\*/u);
  assert.match(report, /track-001/u);
  assert.doesNotMatch(report, /secret-track|\.flac|[/\\]music[/\\]/u);
});
