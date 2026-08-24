import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";

import { EqCurve } from "@/components/EqCurve";
import { Field } from "@/components/Field";
import {
  type AuthoringImportPreview,
  type BackgroundJob,
  MODEL_EQ_DISCLOSURE_VERSION,
  type ModelEqAvailability,
  assistantApi,
  authoringImportApi,
  jobsApi,
} from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { uniqueSlug } from "@/core/slugify";
import { toast } from "@/core/toast";

import { readableBackgroundJobError } from "./backgroundJobs";
import { ProviderBoundaryPopover } from "./AssistantInfoPopover";
import {
  MODEL_EQ_DRAFT_JOB_KIND,
  eqDraftFromJob,
  isEqDraftJobActive,
} from "./eqDraftJobs";
import { ModelUsageSummary } from "./ModelUsageSummary";

interface EqImportDocument {
  schema: "authoring-import/v1";
  name: string;
  presets: Array<{
    id: string;
    name: string;
    description: string;
    effects: Array<{
      type: "eq";
      bands: Array<{ frequency: number; gain: number }>;
    }>;
  }>;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : "The request failed unexpectedly.";
}

function unavailableMessage(reasonCode: string | null): string {
  switch (reasonCode) {
    case "model_quality_not_passed":
      return "Run and pass the EQ quality check in model settings first.";
    case "role_not_enabled":
    case "role_not_configured":
      return "Assign and enable an EQ model in model settings first.";
    case "connection_not_verified":
    case "model_not_tested":
      return "Verify and test the assigned EQ model in model settings first.";
    default:
      return "The connected EQ model is not ready yet.";
  }
}

interface EqAssistantViewProps {
  embedded?: boolean;
  onCreated?: (presetId: string) => void | Promise<void>;
}

export function EqAssistantView({
  embedded = false,
  onCreated,
}: EqAssistantViewProps = {}) {
  const activeModeId = usePlayerStore((state) => state.state?.active_mode_id ?? null);
  const [name, setName] = useState("Warm Tavern");
  const [goal, setGoal] = useState("");
  const [availability, setAvailability] = useState<ModelEqAvailability | null>(null);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [job, setJob] = useState<BackgroundJob | null>(null);
  const [starting, setStarting] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [preview, setPreview] = useState<AuthoringImportPreview | null>(null);
  const [previewDocument, setPreviewDocument] = useState<EqImportDocument | null>(
    null,
  );
  const [created, setCreated] = useState(false);
  const draft = useMemo(() => eqDraftFromJob(job), [job]);
  const activeJob = isEqDraftJobActive(job);
  const previewItem = preview?.items.find((item) => item.kind === "preset") ?? null;

  const clearReview = useCallback(() => {
    setPreview(null);
    setPreviewDocument(null);
    setCreated(false);
  }, []);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function poll() {
      try {
        const [nextAvailability, history] = await Promise.all([
          assistantApi.getModelEqAvailability(),
          jobsApi.list({ kind: MODEL_EQ_DRAFT_JOB_KIND, limit: 1 }),
        ]);
        if (disposed) return;
        setAvailability(nextAvailability);
        setJob(history[0] ?? null);
        setStatusError(null);
        timer = window.setTimeout(() => void poll(),
          isEqDraftJobActive(history[0]) ? 1500 : 5000,
        );
      } catch (error) {
        if (disposed) return;
        setStatusError(errorMessage(error));
        timer = window.setTimeout(() => void poll(), 5000);
      }
    }

    void poll();
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, []);

  async function startDraft(event: FormEvent) {
    event.preventDefault();
    if (!availability?.available || !name.trim() || goal.trim().length < 2) return;
    setStarting(true);
    clearReview();
    try {
      const started = await assistantApi.startModelEqDraft(
        { name: name.trim(), goal: goal.trim() },
        MODEL_EQ_DISCLOSURE_VERSION,
      );
      setJob(started);
    } catch (error) {
      toast.error("EQ draft could not start", errorMessage(error));
    } finally {
      setStarting(false);
    }
  }

  async function cancelDraft() {
    if (job === null || !activeJob) return;
    try {
      setJob(await jobsApi.cancel(job.id));
    } catch (error) {
      toast.error("Cancel failed", errorMessage(error));
    }
  }

  async function reviewDraft() {
    if (draft === null || activeModeId === null) return;
    const description = [
      `Goal: ${draft.goal}`,
      draft.rationale,
      ...draft.cautions.map((item) => `Caution: ${item}`),
    ]
      .join("\n\n")
      .slice(0, 2000);
    const document: EqImportDocument = {
      schema: "authoring-import/v1",
      name: `Assistant EQ draft: ${draft.name}`,
      presets: [
        {
          id: uniqueSlug(draft.name, [], "preset"),
          name: draft.name,
          description,
          effects: [{ type: "eq", bands: draft.bands }],
        },
      ],
    };
    setPreviewing(true);
    clearReview();
    try {
      const nextPreview = await authoringImportApi.previewDocument(
        activeModeId,
        document,
        "Assistant EQ preset builder",
      );
      setPreview(nextPreview);
      setPreviewDocument(document);
    } catch (error) {
      toast.error("Review failed", errorMessage(error));
    } finally {
      setPreviewing(false);
    }
  }

  async function createPreset() {
    if (
      activeModeId === null ||
      previewDocument === null ||
      previewItem?.status !== "ready"
    ) {
      return;
    }
    let createdPresetId: string | null = null;
    setCommitting(true);
    try {
      const result = await authoringImportApi.commitDocument(
        activeModeId,
        previewDocument,
        [{ kind: "preset", resource_id: previewItem.resource_id }],
        "Assistant EQ preset builder",
      );
      if (result.imported.length !== 1) {
        toast.error(
          "Create skipped",
          result.skipped[0]?.reason ?? "The preset was not created.",
        );
        return;
      }
      setCreated(true);
      createdPresetId = previewItem.resource_id;
      toast.success("Preset created", `${draft?.name ?? "The EQ preset"} is ready.`);
    } catch (error) {
      toast.error("Create failed", errorMessage(error));
    } finally {
      setCommitting(false);
    }
    if (createdPresetId !== null && onCreated !== undefined) {
      try {
        await onCreated(createdPresetId);
      } catch (error) {
        toast.warn("Preset created but could not be opened", errorMessage(error));
      }
    }
  }

  return (
    <div
      className={`assistant-playlist-view assistant-eq-view${embedded ? " is-embedded" : ""}`}
    >
      {!embedded ? (
        <header className="assistant-page-header">
          <div>
            <p className="assistant-eyebrow">Connected model · review-first</p>
            <h1>Draft a custom EQ preset</h1>
            <p>
              Describe the sound you want. The model can suggest only bounded ten-band
              EQ gains; you still preview and explicitly create the preset through the
              normal Authoring review.
            </p>
          </div>
          <span className="assistant-algorithm">model-graphic-eq/v2</span>
        </header>
      ) : null}

      <div className="assistant-workbench">
        <aside className="assistant-composer">
          <form className="surface-card assistant-form" onSubmit={startDraft}>
            <Field label="Preset name">
              <input
                value={name}
                maxLength={128}
                disabled={activeJob}
                onChange={(event) => setName(event.target.value)}
              />
            </Field>
            <Field label="Sound goal">
              <textarea
                value={goal}
                maxLength={1000}
                disabled={activeJob}
                placeholder="Warm medieval tavern, more wooden body, softer brittle highs…"
                onChange={(event) => setGoal(event.target.value)}
              />
            </Field>

            {availability !== null ? (
              <ProviderBoundaryPopover
                shared={availability.disclosure.shared_with_provider}
                neverShared={availability.disclosure.never_shared}
                sharedLabel="Shared after you click"
                footer="This request may incur provider cost."
              />
            ) : null}

            <button
              type="submit"
              className="btn-primary"
              disabled={
                starting || activeJob || !availability?.available || goal.trim().length < 2
              }
            >
              {starting ? "Starting…" : activeJob ? "Draft in progress…" : "Create draft"}
            </button>
            {!availability?.available ? (
              <p className="field-hint">
                {statusError ?? unavailableMessage(availability?.reason_code ?? null)}{" "}
                <Link to="/assistant/settings/models">Open model settings</Link>
              </p>
            ) : null}
          </form>
        </aside>

        <main className="assistant-eq-results">
          {activeJob && job !== null ? (
            <section className="surface-card assistant-model-progress">
              <div>
                <strong>{job.progress_phase || "Queued"}</strong>
                <span>{job.progress_message}</span>
              </div>
              {job.progress_total === null ? (
                <progress aria-label="EQ draft progress" />
              ) : (
                <progress
                  aria-label="EQ draft progress"
                  value={job.progress_current}
                  max={Math.max(1, job.progress_total)}
                />
              )}
              <button type="button" className="btn-secondary" onClick={cancelDraft}>
                {job.status === "cancel_requested" ? "Cancelling…" : "Cancel draft"}
              </button>
            </section>
          ) : draft !== null ? (
            <section className="surface-card assistant-eq-draft">
              <div className="assistant-section-heading">
                <div>
                  <p className="assistant-eyebrow">Review-only draft</p>
                  <h2>{draft.name}</h2>
                </div>
                <span>10 bands</span>
              </div>
              <EqCurve bands={draft.bands} height={170} />
              <p>{draft.rationale}</p>
              {draft.cautions.length > 0 ? (
                <div>
                  <strong>Things to check while listening</strong>
                  <ul>
                    {draft.cautions.map((item) => <li key={item}>{item}</li>)}
                  </ul>
                </div>
              ) : null}
              <div className="assistant-eq-band-grid" aria-label="Suggested EQ gains">
                {draft.bands.map((band) => (
                  <span key={band.frequency}>
                    <strong>{band.frequency >= 1000 ? `${band.frequency / 1000}k` : band.frequency} Hz</strong>
                    {band.gain > 0 ? "+" : ""}{band.gain.toFixed(1)} dB
                  </span>
                ))}
              </div>
              <ModelUsageSummary job={job} />
              <button
                type="button"
                className="btn-primary"
                disabled={previewing || activeModeId === null}
                onClick={() => void reviewDraft()}
              >
                {previewing ? "Preparing review…" : "Review preset"}
              </button>
              {activeModeId === null ? (
                <p className="field-hint">Select a mode before reviewing this preset.</p>
              ) : null}

              {previewItem !== null ? (
                <div className="assistant-eq-import-review">
                  <strong>
                    {previewItem.status === "ready"
                      ? "Ready to create"
                      : previewItem.status === "conflict"
                        ? "A preset with this ID already exists"
                        : "The draft needs changes"}
                  </strong>
                  <p>{previewItem.reason ?? previewItem.summary}</p>
                  {previewItem.issues.map((issue) => (
                    <p key={`${issue.code}:${issue.message}`}>{issue.message}</p>
                  ))}
                  <button
                    type="button"
                    className="btn-primary"
                    disabled={committing || created || previewItem.status !== "ready"}
                    onClick={() => void createPreset()}
                  >
                    {created
                      ? "Preset created"
                      : committing
                        ? "Creating…"
                        : embedded
                          ? "Create and continue editing"
                          : "Create preset"}
                  </button>
                </div>
              ) : null}
            </section>
          ) : job?.status === "failed" ? (
            <section className="surface-card assistant-analysis-error" role="alert">
              <strong>The EQ draft did not finish</strong>
              <span>{readableBackgroundJobError(job.error, "Try the request again.")}</span>
            </section>
          ) : (
            <section className="surface-card assistant-provider-empty">
              <h2>Your draft will appear here</h2>
              <p>No preset is written until you preview and confirm the import.</p>
            </section>
          )}
        </main>
      </div>
    </div>
  );
}
