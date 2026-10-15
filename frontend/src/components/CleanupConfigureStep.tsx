import { CleanupEvidenceImport } from "@/components/CleanupEvidence";
import { MAX_CATALOG_TRACKS, RULE_GROUPS } from "@/components/cleanupRules";
import type { CleanupConfig } from "@/components/cleanupRules";
import type { CleanupRuleId } from "@/core/api";

interface CleanupConfigureStepProps {
  config: CleanupConfig;
  onChange: (patch: Partial<CleanupConfig>) => void;
  /** Currently-browsed folder ("" = music root). */
  path: string;
  selectedCount: number;
}

export function CleanupConfigureStep({
  config,
  onChange,
  path,
  selectedCount,
}: CleanupConfigureStepProps) {
  const { scopeType, recursive, rules, useCatalogs, refreshCatalogs, imports } = config;

  function toggleRule(id: CleanupRuleId) {
    const next = new Set(rules);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    onChange({ rules: next });
  }

  return (
    <div className="cleanup-options">
      <section>
        <h3 className="section-label">Where to look</h3>
        <div className="cleanup-scope">
          <label className="cleanup-choice">
            <input
              type="radio"
              name="cleanup-scope"
              checked={scopeType === "all"}
              onChange={() => onChange({ scopeType: "all" })}
            />
            <span>Entire library</span>
          </label>
          <label className="cleanup-choice">
            <input
              type="radio"
              name="cleanup-scope"
              checked={scopeType === "folder"}
              onChange={() => onChange({ scopeType: "folder" })}
            />
            <span>
              Current folder <strong>{path || "(root)"}</strong>
            </span>
            {scopeType === "folder" ? (
              <label className="cleanup-subchoice">
                <input
                  type="checkbox"
                  checked={recursive}
                  onChange={(e) => onChange({ recursive: e.target.checked })}
                />
                <span className="muted">include subfolders</span>
              </label>
            ) : null}
          </label>
          <label className={`cleanup-choice${selectedCount === 0 ? " disabled" : ""}`}>
            <input
              type="radio"
              name="cleanup-scope"
              disabled={selectedCount === 0}
              checked={scopeType === "tracks"}
              onChange={() => onChange({ scopeType: "tracks" })}
            />
            <span>
              Selected tracks{" "}
              <span className="muted">
                ({selectedCount === 0 ? "none ticked in the list" : selectedCount})
              </span>
            </span>
          </label>
        </div>
      </section>
      {RULE_GROUPS.map((group) => (
        <section key={group.label}>
          <h3 className="section-label">{group.label}</h3>
          <div className="cleanup-rules">
            {group.rules.map((rule) => (
              <label key={rule.id} className="cleanup-choice">
                <input
                  type="checkbox"
                  checked={rules.has(rule.id)}
                  onChange={() => toggleRule(rule.id)}
                />
                <span>
                  {rule.label}
                  <span className="cleanup-hint muted">{rule.hint}</span>
                </span>
              </label>
            ))}
          </div>
        </section>
      ))}
      <section>
        <h3 className="section-label">Catalog evidence</h3>
        <label className="cleanup-choice cleanup-catalog-choice">
          <input
            type="checkbox"
            checked={useCatalogs}
            onChange={(event) => onChange({ useCatalogs: event.target.checked })}
          />
          <span>
            Identify tracks and retrieve canonical metadata
            <span className="cleanup-hint muted">
              Uses the enabled Sources connectors. Ambiguous matches make no proposal; catalog
              repairs and community mood tags always start unticked. MusicBrainz receives title,
              artist, album, duration, and embedded or imported catalog identifiers; AcoustID receives a local fingerprint and duration;
              Last.fm receives the identified recording ID. Library paths and audio files are
              never uploaded.
            </span>
          </span>
        </label>
        {useCatalogs && <>
          <p className="cleanup-hint muted">Catalog lookup supports up to {MAX_CATALOG_TRACKS} tracks per run. Larger scans still produce local cleanup suggestions.</p>
          <label className="cleanup-choice"><input type="checkbox" checked={refreshCatalogs} onChange={(event) => onChange({ refreshCatalogs: event.target.checked })} />Refresh catalog results, including previous no-match results</label>
          <CleanupEvidenceImport imports={imports} onChange={(next) => onChange({ imports: next })} />
        </>}
      </section>
      <p className="muted small">
        Nothing is changed yet — the next step shows every proposed fix as a
        diff for you to confirm. File, folder, and embedded-tag changes are
        journaled and can be reverted from History; accepted community mood
        tags remain database-only operator tags.
      </p>
    </div>
  );
}
