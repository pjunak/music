import { toast } from "@/core/toast";

import { downloadJson } from "./downloadJson";

export function TaggingRunExport({ id, result }: { id: string; result: Record<string, unknown> | null }) {
  const progress = result?.feature_progress;
  const rows = result?.track_results ?? (progress && typeof progress === "object" ? (progress as Record<string, unknown>).track_results : null) ?? [];
  if (!Array.isArray(rows)) return null;
  function download() {
    try {
      const data = { schema_version: "assistant-mood-run-export/v1", run_id: id, track_results: rows, usage: result?.usage ?? null };
      downloadJson(data, "mood-run-" + id.replace(/[^a-zA-Z0-9-]/g, "").slice(0, 64) + ".json");
    } catch {
      toast.error("Could not export this run");
    }
  }
  return <button type="button" className="btn-ghost" onClick={download}>Export retained run results</button>;
}
