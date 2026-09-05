import { describe, expect, it } from "vitest";
import type { BackgroundJob } from "@/core/api";
import { qualityEvidenceNotes } from "./modelQualityUi";

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
