import { describe, expect, it } from "vitest";
import type { BackgroundJob } from "@/core/api";
import type { ModelQualityEvaluation } from "@/core/assistantProvidersApi";
import { modelQualityView, qualityEvidenceNotes, qualityStatusLabel } from "./modelQualityUi";

function job(evaluation: Record<string, unknown>): BackgroundJob {
  return {
    id: "quality", kind: "assistant.model-evaluation.music-tagging-quality-v1",
    status: "succeeded", parameters: {}, result: { evaluation }, error: null,
    progress_current: 1, progress_total: 1, progress_phase: "Complete", progress_message: "",
    attempts: 1, retry_of_id: null, created_at: "2026-09-05T12:00:00Z",
    updated_at: "2026-09-05T12:00:01Z", started_at: "2026-09-05T12:00:00Z",
    finished_at: "2026-09-05T12:00:01Z",
  };
}

describe("quality evidence diagnostics", () => {
  it("explains the whole-case requirement behind the context-only failure", () => {
    const notes = qualityEvidenceNotes(job({
      minimum_quality_pass_rate: 0.9,
      context_only_results: { passed: false, passed_cases: 8, total_cases: 9 },
    }));
    expect(notes[0]).toMatchObject({ tone: "failure" });
    expect(notes[0]?.message).toContain("Requires 9/9 at the 90% threshold");
  });

  it("retains model evidence for empty answers and semantic misses in safety reruns", () => {
    const notes = qualityEvidenceNotes(job({ cases: [{
      id: "settled", description: "Steady texture", passed: false,
      tags: [], failures: ["Missing required tags: calm"],
      evidence: ["The measurements do not establish an emotional tone."],
      safety_repeat_tags: ["calm"], safety_repeat_failures: ["Missing required tags: rest"],
      safety_repeat_evidence: ["Low onset activity and stable sections."],
    }] }));
    expect(notes).toHaveLength(2);
    expect(notes[0]?.message).toContain("returned no tags. Model-reported evidence: The measurements");
    expect(notes[1]?.message).toContain("(safety rerun): returned calm");
  });

  it("explains why a vocabulary group fails despite a high aggregate score", () => {
    const notes = qualityEvidenceNotes(job({ passed_cases: 55, total_cases: 56,
      vocabulary_results: [
        { vocabulary: "default", passed: true, passed_cases: 50, total_cases: 50 },
        { vocabulary: "custom", passed: false, passed_cases: 4, total_cases: 5 },
        { vocabulary: "maximum", passed: true, passed_cases: 1, total_cases: 1 },
      ],
    }));
    expect(notes).toHaveLength(3);
    expect(notes[1]).toMatchObject({ tone: "failure", message: "Custom vocabulary: 4/5 scenarios; failed its independent quality gate." });
  });

  it("separates missing provider inputs from a failed model ranking", () => {
    const notes = qualityEvidenceNotes(job({ cases: [{ id: "pool", description: "Study scene", passed: false,
      failures: ["model_execution_timeout"],
      candidate_recall: { pool_tracks: 15, relevant_tracks: 2, relevant_in_pool: 1 },
    }] }));
    expect(notes[0]?.message).toContain("local candidate preparation supplied 1/2 relevant tracks");
    expect(notes[0]?.message).toContain("cannot rank tracks absent from its input");
  });

  it("keeps historic reports readable and ignores inconsistent new metrics", () => {
    expect(qualityEvidenceNotes(job({ cases: [{ id: "old", passed: false }] }))).toEqual([]);
    expect(qualityEvidenceNotes(job({
      vocabulary_results: [{ vocabulary: "custom", passed: true, passed_cases: 6, total_cases: 5 }],
      cases: [{ id: "bad", description: "Bad metrics", candidate_recall: { pool_tracks: 1, relevant_tracks: 5, relevant_in_pool: 3 } }],
    }))).toEqual([]);
  });
});

describe("quality scenario counters", () => {
  const evaluation: ModelQualityEvaluation = {
    evaluation_id: "music-tagging-quality-v1", role_id: "music_tagger",
    label: "Mood tagging", description: "Synthetic scenarios", status: "never",
    suite_id: "fixture", passed_cases: 0, total_cases: 0,
    last_job_id: null, last_evaluated_at: null,
  };

  it("uses the same scenario total during primary checks, safety reruns and the completed verdict", () => {
    const running = { ...job({}), status: "running" as const,
      progress_current: 50, progress_total: 63 };
    const label = (current: BackgroundJob, report = evaluation) =>
      qualityStatusLabel(report, modelQualityView(report, undefined, [current]), false);
    expect(label(running)).toBe("50 / 63 checked");
    expect(label({ ...running, progress_current: 62 })).toBe("62 / 63 checked");
    const done = { ...running, status: "succeeded" as const, progress_current: 63 };
    expect(label(done, { ...evaluation, status: "failed", last_job_id: done.id,
      passed_cases: 59, total_cases: 63 })).toBe("59 / 63 passed");
  });

  it("labels a diagnostic retest subset explicitly without implying full certification", () => {
    const running = { ...job({}), status: "running" as const,
      parameters: { case_ids: ["one", "two"] }, progress_current: 1, progress_total: 2 };
    expect(qualityStatusLabel(evaluation, modelQualityView(evaluation, undefined, [running]), false))
      .toBe("Rechecking 1 / 2 scenarios");
    const cancelled = { ...running, status: "cancel_requested" as const };
    expect(qualityStatusLabel(evaluation, modelQualityView(evaluation, undefined, [cancelled]), false))
      .toBe("Cancelling");
  });
});
