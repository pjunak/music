import { Link } from "react-router-dom";

import { EmptyState } from "@/components/EmptyState";

export function AssistantCleanupView() {
  return (
    <div className="assistant-placeholder">
      <div className="surface-card assistant-placeholder-card">
        <EmptyState
          title="Cleanup stays review-first"
          action={
            <Link className="btn-link" to="/library">
              Open Library cleanup
            </Link>
          }
        >
          The existing cleanup tools remain in Library until the Assistant has a
          durable analysis-job foundation. This avoids duplicating or weakening a
          workflow that already works.
        </EmptyState>
      </div>
    </div>
  );
}
