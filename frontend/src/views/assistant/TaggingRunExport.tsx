import { toast } from "@/core/toast";

export function TaggingRunExport({ id, result }: { id: string; result: Record<string, unknown> | null }) {
  const progress = result?.feature_progress;
  const rows = result?.track_results ?? (progress && typeof progress === "object" ? (progress as Record<string, unknown>).track_results : null);
  if (!Array.isArray(rows) || rows.length === 0) return null;
  function download() {
    try {
      const data = { schema_version: "assistant-mood-run-export/v1", run_id: id, track_results: rows, usage: result?.usage ?? null };
      const url = URL.createObjectURL(new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }));
      const anchor = document.createElement("a");
      anchor.href = url;
      anchor.download = `mood-run-${id.replace(/[^a-zA-Z0-9-]/g, "").slice(0, 64)}.json`;
      anchor.click();
      window.setTimeout(() => URL.revokeObjectURL(url), 1_000);
    } catch {
      toast.error("Could not export this run");
    }
  }
  return <button type="button" className="btn-ghost" onClick={download}>Export retained run results</button>;
}
