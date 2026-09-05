import type { TagReviewSummary as Summary } from "@/core/api";

function isCount(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isSummary(value: unknown): value is Summary {
  if (typeof value !== "object" || value === null) return false;
  const summary = value as Partial<Summary>;
  if (!isCount(summary.matching_tracks) || !Array.isArray(summary.sources)) return false;
  const ids = new Set<string>();
  return summary.sources.every((source: unknown) => {
    if (typeof source !== "object" || source === null) return false;
    const row = source as Partial<Summary["sources"][number]>;
    if (
      typeof row.analyzer_id !== "string" ||
      !row.analyzer_id.trim() ||
      ids.has(row.analyzer_id)
    ) return false;
    ids.add(row.analyzer_id);
    return isCount(row.pending) && isCount(row.accepted) && isCount(row.rejected);
  });
}

export function TagReviewSummary({ summary }: { summary: Summary | undefined }) {
  // Older servers omit this field. Invalid responses must not become apparent zero counts.
  if (!isSummary(summary)) return null;
  const reviewed = summary.sources.reduce(
    (sum, row) => sum + row.accepted + row.rejected, 0,
  );
  const total = summary.sources.reduce(
    (sum, row) => sum + row.pending + row.accepted + row.rejected, 0,
  );
  if (!isCount(total) || !isCount(reviewed)) return null;
  return (
    <details className="assistant-tag-review-summary">
      <summary>Review summary · {reviewed} of {total} suggestions reviewed</summary>
      <p className="muted">
        Current suggestions across {summary.matching_tracks} matching tracks, including all pages
        and review states. Counts describe your decisions, not model accuracy or lifetime history.
      </p>
      {total === 0 ? (
        <p>No current suggestions in this scope.</p>
      ) : (
        <div className="assistant-tag-review-summary-scroll">
          <table aria-label="Current suggestion review counts">
            <thead>
              <tr>
                <th scope="col">Source</th>
                <th scope="col">Pending</th>
                <th scope="col">Accepted</th>
                <th scope="col">Rejected</th>
              </tr>
            </thead>
            <tbody>
              {summary.sources.map((row) => (
                <tr key={row.analyzer_id}>
                  <th scope="row">{row.analyzer_id}</th>
                  <td>{row.pending}</td>
                  <td>{row.accepted}</td>
                  <td>{row.rejected}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </details>
  );
}
