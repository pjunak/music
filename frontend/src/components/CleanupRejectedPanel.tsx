import { useEffect, useState } from "react";
import { cleanupApi } from "@/core/api";
import type { CleanupRejectedPage, CleanupReviewProposal } from "@/core/api";

function ProposalEvidence({ proposal }: { proposal: CleanupReviewProposal }) {
  const evidence = proposal.evidence;
  const recording = evidence?.recording_id ?? (evidence?.entity === "recording" ? evidence.id : undefined);
  const release = evidence?.release_id;
  return <details>
    <summary>Suggestion evidence</summary>
    <p>{evidence ? "The catalog records used for this suggestion are retained for comparison." : "Based on file, tag and folder evidence at the time of review."}</p>
    {recording && <p><a href={`https://musicbrainz.org/recording/${encodeURIComponent(recording)}`} target="_blank" rel="noreferrer">View recording in MusicBrainz</a></p>}
    {release && <p><a href={`https://musicbrainz.org/release/${encodeURIComponent(release)}`} target="_blank" rel="noreferrer">View album edition in MusicBrainz</a></p>}
  </details>;
}

export function CleanupRejectedPanel({ onRestore, onRecheck }: {
  onRestore: (proposal: CleanupReviewProposal) => void;
  onRecheck: (proposal: CleanupReviewProposal) => void;
}) {
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [before, setBefore] = useState<number | null>(null);
  const [page, setPage] = useState<CleanupRejectedPage>({ items: [], next_before: null });
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [revision, setRevision] = useState(0);
  useEffect(() => {
    let active = true;
    setLoading(true); setError(null);
    void cleanupApi.rejected(before, search).then((result) => { if (active) setPage(result); })
      .catch((cause: unknown) => { if (active) setError(cause instanceof Error ? cause.message : "Could not load rejected suggestions."); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, [before, search, revision]);

  async function restore(id: number) {
    setBusy(true); setError(null);
    try { onRestore(await cleanupApi.restoreRejected(id)); }
    catch (cause) { setError(cause instanceof Error ? cause.message : "Could not restore this suggestion."); }
    finally { setBusy(false); }
  }
  async function forget(id: number) {
    setBusy(true); setError(null);
    try { await cleanupApi.forgetRejected(id); setRevision((value) => value + 1); }
    catch (cause) { setError(cause instanceof Error ? cause.message : "Could not remove this rejection."); }
    finally { setBusy(false); }
  }
  return <div className="cleanup-rejected-pool">
    <p>Rejected file and metadata suggestions stay here. Restore a suggestion to review it and choose whether to apply it. Leaving a checkbox unticked does not reject it.</p>
    <form className="cleanup-rejected-search" onSubmit={(event) => { event.preventDefault(); setBefore(null); setSearch(query); setRevision((value) => value + 1); }}>
      <label>Search rejected suggestions<input type="search" value={query} maxLength={256} onChange={(event) => setQuery(event.target.value)} placeholder="Path, field, or value" /></label>
      <button type="submit" disabled={busy}>Search</button>
    </form>
    {error && <div role="alert">{error}<button type="button" className="btn-link" disabled={busy} onClick={() => setRevision((value) => value + 1)}>Retry</button></div>}
    {loading ? <p role="status">Loading rejected suggestions…</p> : !error && page.items.length === 0 ? <p>{search ? "No rejected suggestions match this search." : "No rejected suggestions."}</p> : null}
    {!loading && page.items.map((item) => <article key={item.id} className="cleanup-rejected-item">
      <h3>{item.proposal.path}</h3>
      <p className="muted small">{item.proposal.field ?? (item.proposal.kind === "folder_rename" ? "Folder name" : "File name")} · {item.proposal.evidence ? item.proposal.rules.includes("model_catalog_choice") ? "AI candidate review" : "MusicBrainz" : "Local"} · Rejected {new Date(item.rejected_at * 1000).toLocaleString()}</p>
      <p className="cleanup-diff"><span className="cleanup-old">{item.proposal.old === null || item.proposal.old === "" ? "(empty)" : String(item.proposal.old)}</span><span aria-hidden="true"> → </span><span className="cleanup-new">{item.proposal.new === null || item.proposal.new === "" ? "(empty)" : String(item.proposal.new)}</span></p>
      <ProposalEvidence proposal={item.proposal} />
      {!item.current && <p className="muted">The file, metadata, or evidence has changed. Check again before applying a correction.</p>}
      <div className="cleanup-rejected-actions">
        {item.current ? <button type="button" disabled={busy} onClick={() => void restore(item.id)}>Restore to review</button> : <button type="button" disabled={busy} onClick={() => onRecheck(item.proposal)}>Check again</button>}
        <button type="button" className="btn-link" disabled={busy} onClick={() => void forget(item.id)}>Remove rejection</button>
      </div>
    </article>)}
    <div className="cleanup-rejected-actions">
      {before !== null && <button type="button" disabled={busy || loading} onClick={() => setBefore(null)}>Newest</button>}
      {page.next_before !== null && <button type="button" disabled={busy || loading} onClick={() => setBefore(page.next_before)}>Older suggestions</button>}
    </div>
  </div>;
}
