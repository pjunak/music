import { describe, expect, it } from "vitest";

import type { BackgroundJob } from "@/core/api";

import { eqDraftFromJob, isEqDraftJobActive } from "./eqDraftJobs";

function job(overrides: Partial<BackgroundJob> = {}): BackgroundJob {
  return {
    id: "eq-job-1",
    kind: "assistant.model-eq-draft",
    status: "succeeded",
    parameters: {},
    result: {
      schema_version: "assistant-eq-draft-job-result/v1",
      draft: {
        name: "Warm Tavern",
        goal: "warm wooden body",
        bands: [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000].map(
          (frequency) => ({ frequency, gain: frequency === 250 ? 2 : 0 }),
        ),
        rationale: "A small low-mid lift adds warmth.",
        cautions: ["Check headroom."],
      },
    },
    error: null,
    progress_current: 2,
    progress_total: 2,
    progress_phase: "Draft ready",
    progress_message: "Ready for review",
    attempts: 1,
    retry_of_id: null,
    created_at: "2026-08-19T10:00:00Z",
    updated_at: "2026-08-19T10:00:01Z",
    started_at: "2026-08-19T10:00:00Z",
    finished_at: "2026-08-19T10:00:01Z",
    ...overrides,
  };
}

describe("EQ draft jobs", () => {
  it("restores a completed strict draft after refresh", () => {
    const draft = eqDraftFromJob(job());

    expect(draft?.name).toBe("Warm Tavern");
    expect(draft?.bands).toHaveLength(10);
    expect(draft?.bands[3]).toEqual({ frequency: 250, gain: 2 });
  });

  it("rejects incomplete or malformed stored results", () => {
    expect(eqDraftFromJob(job({ result: { schema_version: "wrong" } }))).toBeNull();
    expect(
      eqDraftFromJob(
        job({
          result: {
            schema_version: "assistant-eq-draft-job-result/v1",
            draft: {
              name: "Broken",
              goal: "broken",
              rationale: "broken",
              cautions: [],
              bands: [{ frequency: 32, gain: 0 }],
            },
          },
        }),
      ),
    ).toBeNull();
  });

  it("rejects non-canonical frequencies, gain ranges, and gain steps", () => {
    const base = job().result as {
      schema_version: string;
      draft: {
        name: string;
        goal: string;
        bands: Array<{ frequency: number; gain: number }>;
        rationale: string;
        cautions: string[];
      };
    };
    for (const replacement of [
      { frequency: 63, gain: 0 },
      { frequency: 64, gain: 12.5 },
      { frequency: 64, gain: 0.25 },
    ]) {
      const bands = base.draft.bands.map((band) => ({ ...band }));
      bands[1] = replacement;
      expect(
        eqDraftFromJob(
          job({
            result: {
              ...base,
              draft: { ...base.draft, bands },
            },
          }),
        ),
      ).toBeNull();
    }
  });

  it("treats queued and running jobs as active", () => {
    expect(isEqDraftJobActive(job({ status: "queued" }))).toBe(true);
    expect(isEqDraftJobActive(job({ status: "running" }))).toBe(true);
    expect(isEqDraftJobActive(job())).toBe(false);
  });
});
