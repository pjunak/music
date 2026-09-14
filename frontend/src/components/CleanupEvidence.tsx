import { useState } from "react";
import type { CleanupEnrichmentPlan, CleanupImportedEvidence } from "@/core/api";
import { toast } from "@/core/toast";
import { releaseIdFromInput } from "./cleanupReview";

export function CleanupEvidence({ plan }: { plan: CleanupEnrichmentPlan }) {
  return <div className="cleanup-evidence">
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
  function changeSource(value: string) {
    onChange(imports.map((item) => {
      if (value.trim()) return { ...item, source: value, propose: item.propose === true };
      const updated = { ...item, propose: false };
      delete updated.source;
      return updated;
    }));
  }
  return <div>
    <label className="cleanup-evidence-input">Import metadata evidence (JSON)
      <input type="file" accept="application/json,.json" onChange={async (event) => {
        const file = event.target.files?.[0];
        if (!file) return;
        try {
          if (file.size > 1_000_000) throw new Error("The evidence file must be smaller than 1 MB.");
          const parsed: unknown = JSON.parse(await file.text());
          if (!parsed || typeof parsed !== "object" || !("tracks" in parsed) || !Array.isArray(parsed.tracks) || parsed.tracks.length > 500) throw new Error("Expected a JSON object containing a tracks array (at most 500 tracks).");
          for (const item of parsed.tracks as unknown[]) {
            if (!item || typeof item !== "object" || !("track_id" in item) || !Number.isSafeInteger(item.track_id) || !("fields" in item) || !item.fields || typeof item.fields !== "object" || Array.isArray(item.fields) || Object.values(item.fields).some((v) => typeof v !== "string" || v.length > 512)) throw new Error("Each track needs a numeric track_id and a fields object containing text values.");
            if ("source" in item && (typeof item.source !== "string" || !item.source.trim() || new TextEncoder().encode(item.source).length > 512 || Array.from(item.source).some((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) >= 127 && c.charCodeAt(0) <= 159))) throw new Error("The source reference must be nonempty text up to 512 bytes.");
            if ("propose" in item && (typeof item.propose !== "boolean" || item.propose && !("source" in item))) throw new Error("Review proposals require a source reference and a boolean propose value.");
          }
          onChange(parsed.tracks as CleanupImportedEvidence[]);
        } catch (error) { toast.error("Evidence import failed", error instanceof Error ? error.message : undefined); }
      }} />
    </label>
    <p className="muted small">Selected data only; no scripts or audio processing. Format: {`{"tracks":[{"track_id":7,"fields":{"recording_mbid":"…","release_mbid":"…"}}]}`}. Track IDs appear in review evidence. Imported tracks must belong to the selected scope.</p>
    {imports.length > 0 && <>
      <p>{imports.length} imported tracks <button type="button" className="btn-link" onClick={() => onChange([])}>Clear import</button></p>
      <label className="cleanup-evidence-input">Source reference for imported metadata
        <input type="text" maxLength={512} value={imports.every((item) => item.source === imports[0]?.source) ? imports[0]?.source ?? "" : ""} placeholder="Album page, booklet, or creator manifest" onChange={(event) => changeSource(event.target.value)} />
      </label>
      <label className="cleanup-evidence-toggle"><input type="checkbox" checked={imports.every((item) => item.propose === true)} disabled={imports.some((item) => !item.source?.trim())} onChange={(event) => onChange(imports.map((item) => ({ ...item, propose: event.target.checked })))} />Propose imported metadata changes for review</label>
      <p className="muted small">Use this for a confirmed edition or creator tracklist. Title, artist, composer, album, album artist, genre, track/disc numbers, release date and original release date become optional suggestions, including when catalog identification fails. Use date and original_date with YYYY, YYYY-MM or YYYY-MM-DD in the JSON; identifiers remain evidence. Check the file mapping and edition before applying.</p>
    </>}
  </div>;
}

export function CleanupEditionTarget({ folder, disabled, onTarget }: { folder: string; disabled: boolean; onTarget: (id: string) => void }) {
  const [value, setValue] = useState("");
  const id = releaseIdFromInput(value);
  return <details className="cleanup-evidence">
    <summary>Find a known album edition</summary>
    <label className="cleanup-evidence-input">MusicBrainz release URL or ID for {folder || "this folder"}<input type="text" value={value} maxLength={256} onChange={(event) => setValue(event.target.value)} /></label>
    <p className="muted small">Prepare another lookup for the reviewed tracks in this folder, including editions outside the catalog shortlist. This replaces previous JSON imports for the new lookup. Recording and tracklist checks still apply. This prepares the next review without changing files.</p>
    {value.trim() && !id && <p role="alert">Enter a MusicBrainz release ID or its https://musicbrainz.org/release/ URL.</p>}
    <button type="button" className="btn-ghost" disabled={disabled || !id} onClick={() => { if (id) onTarget(id); }}>Prepare edition lookup</button>
  </details>;
}
