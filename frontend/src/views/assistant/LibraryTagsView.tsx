import { lazy, Suspense } from "react";
import { useSearchParams } from "react-router-dom";
import type { ModelTagFilter, TagSuggestionSource } from "@/core/api";

const LibraryTagEditor = lazy(async () => {
  const module = await import("./LibraryTagEditor");
  return { default: module.LibraryTagEditor };
});

export function LibraryTagsView() {
  const [params] = useSearchParams();
  const status = params.get("model_status") ?? "";
  const source = params.get("suggestion_source") ?? "";
  const jobId = params.get("model_job_id") ?? "";
  return (
    <Suspense
      fallback={
        <div className="library-view assistant-context-view assistant-tags-view">
          <p className="muted">Loading mood tags…</p>
        </div>
      }
    >
      <LibraryTagEditor
        key={params.toString()}
        initialModelFilter={(["processed", "current", "stale", "missing", "with_suggestions", "without_suggestions"].includes(status) ? status : "") as "" | ModelTagFilter}
        initialSourceFilter={(["model", "metadata", "catalog"].includes(source) ? source : "") as "" | TagSuggestionSource}
        initialModelJobId={jobId}
      />
    </Suspense>
  );
}
