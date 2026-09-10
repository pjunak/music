import type { CleanupEnrichmentPlan, CleanupImportedEvidence } from "@/core/api";
import { toast } from "@/core/toast";

export function CleanupEvidence({ plan, edition, onEdition }: {
  plan: CleanupEnrichmentPlan;
  edition: string;
  onEdition: (id: string) => void;
}) {
  return <div className="cleanup-evidence">
    {(plan.release_choices?.length ?? 0) > 0 && <label>
      Album edition for this folder
      <select value={edition} onChange={(event) => onEdition(event.target.value)}>
        <option value="">Keep edition unresolved</option>
        {plan.release_choices?.map((release) => <option key={release.id} value={release.id}>
          {release.title} · {release.date ?? "date unknown"} · {release.country ?? "country unknown"} · {release.catalog_numbers.join(", ") || release.id.slice(0, 8)} · {release.assignment.matched}/{release.assignment.considered} tracks matched
        </option>)}
      </select>
      <span className="muted small">Changes the proposed edition for tracks in this folder with that release available. New proposals start unchecked.</span>
    </label>}
    <details>
      <summary>Evidence and alternatives · track {plan.track_id}</summary>
      {plan.retrieved_at && <p>Retrieved {new Date(plan.retrieved_at * 1000).toLocaleString()}</p>}
      {plan.recording_observations?.first_release_date && <p>Recording first released: {plan.recording_observations.first_release_date} (separate from edition year)</p>}
      {(plan.recording_observations?.credits.length ?? 0) > 0 && <p>Credits: {plan.recording_observations?.credits.join("; ")}</p>}
      {(plan.local_evidence?.observations.length ?? 0) > 0 && <table>
        <thead><tr><th>Source</th><th>Field</th><th>Observed value</th></tr></thead>
        <tbody>{plan.local_evidence?.observations.map((item, index) => <tr key={index}>
          <td>{item.source}</td><td>{item.field.replaceAll("_", " ")}</td><td>{item.value}</td>
        </tr>)}</tbody>
      </table>}
      {(plan.candidates?.length ?? 0) > 0 && <table>
        <thead><tr><th>Catalog candidate</th><th>Duration</th><th>Search score</th></tr></thead>
        <tbody>{plan.candidates?.map((candidate) => <tr key={candidate.id}>
          <td><a href={`https://musicbrainz.org/recording/${encodeURIComponent(candidate.id)}`} target="_blank" rel="noreferrer">{candidate.artist} — {candidate.title}</a></td>
          <td>{candidate.length_ms === null ? "unknown" : `${(candidate.length_ms / 1000).toFixed(1)} s`}</td>
          <td>{candidate.provider_score.toFixed(2)} (not a probability)</td>
        </tr>)}</tbody>
      </table>}
    </details>
  </div>;
}

export function CleanupEvidenceImport({ imports, onChange }: {
  imports: CleanupImportedEvidence[];
  onChange: (items: CleanupImportedEvidence[]) => void;
}) {
  return <div>
    <label>Import metadata evidence (JSON)
      <input type="file" accept="application/json,.json" onChange={async (event) => {
        const file = event.target.files?.[0];
        if (!file) return;
        try {
          if (file.size > 1_000_000) throw new Error("The evidence file must be smaller than 1 MB.");
          const parsed: unknown = JSON.parse(await file.text());
          if (!parsed || typeof parsed !== "object" || !("tracks" in parsed) || !Array.isArray(parsed.tracks) || parsed.tracks.length > 500) throw new Error("Expected a JSON object containing a tracks array (at most 500 tracks).");
          for (const item of parsed.tracks as unknown[]) {
            if (!item || typeof item !== "object" || !("track_id" in item) || !Number.isSafeInteger(item.track_id) || !("fields" in item) || !item.fields || typeof item.fields !== "object" || Array.isArray(item.fields) || Object.values(item.fields).some((v) => typeof v !== "string" || v.length > 512)) throw new Error("Each track needs a numeric track_id and a fields object containing text values.");
          }
          onChange(parsed.tracks as CleanupImportedEvidence[]);
        } catch (error) { toast.error("Evidence import failed", error instanceof Error ? error.message : undefined); }
      }} />
    </label>
    <p className="muted small">Selected data only; no scripts or audio processing. Format: {`{"tracks":[{"track_id":7,"fields":{"recording_mbid":"…","release_mbid":"…"}}]}`}. Track IDs appear in review evidence. Imported tracks must belong to the selected scope.</p>
    {imports.length > 0 && <p>{imports.length} imported tracks <button type="button" className="btn-link" onClick={() => onChange([])}>Clear import</button></p>}
  </div>;
}
