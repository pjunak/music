import { EmptyState } from "@/components/EmptyState";

export function LibraryAnalysisView() {
  return (
    <div className="assistant-placeholder">
      <div className="surface-card assistant-placeholder-card">
        <EmptyState title="Audio analysis is not installed yet">
          The playlist builder currently uses only your existing tags, paths, and
          BPM values. Server-side audio analysis will be added here later as an
          optional, durable background job with progress that survives a refresh.
        </EmptyState>
      </div>
    </div>
  );
}
