import { Link } from "react-router-dom";

import { EmptyState } from "@/components/EmptyState";

export function AssistantCleanupView() {
  return (
    <div className="assistant-placeholder">
      <div className="surface-card assistant-placeholder-card">
        <EmptyState
          title="Cleanup remains in Library"
          action={
            <Link className="btn-link" to="/library">
              Open Library cleanup
            </Link>
          }
        >
          Durable jobs and review tools are now available, but moving the existing
          cleanup screen would add no capability by itself. It remains in Library
          until an optional Assistant cleanup pass has its own preview-only contract.
        </EmptyState>
      </div>
    </div>
  );
}
