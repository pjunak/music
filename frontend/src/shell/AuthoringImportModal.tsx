import { useEffect, useMemo, useState } from "react";

import { Modal } from "@/components/Modal";
import { authoringImportApi, modesApi } from "@/core/api";
import type {
  AuthoringImportItem,
  AuthoringImportKind,
  AuthoringImportPreview,
  AuthoringImportResult,
} from "@/core/api";
import { toast } from "@/core/toast";
import type { ModeSummary } from "@/core/types";

const GROUPS: { kind: AuthoringImportKind; label: string }[] = [
  { kind: "playlist", label: "Playlists" },
  { kind: "soundboard", label: "Soundboards" },
  { kind: "interrupt", label: "Interrupts" },
  { kind: "preset", label: "EQ presets" },
  { kind: "cue", label: "Cues" },
];

function itemKey(item: Pick<AuthoringImportItem, "kind" | "resource_id">) {
  return `${item.kind}:${item.resource_id}`;
}

export function AuthoringImportModal({
  open,
  targetModeId,
  onClose,
  onImported,
}: {
  open: boolean;
  targetModeId: string;
  onClose: () => void;
  onImported: (result: AuthoringImportResult) => void;
}) {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const [sourceModeId, setSourceModeId] = useState("");
  const [preview, setPreview] = useState<AuthoringImportPreview | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loadingModes, setLoadingModes] = useState(false);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setModes([]);
    setSourceModeId("");
    setPreview(null);
    setSelected(new Set());
    setError(null);
    setLoadingModes(true);
    setCommitting(false);

    void modesApi
      .list()
      .then((allModes) => {
        if (cancelled) return;
        const sources = allModes.filter((mode) => mode.id !== targetModeId);
        setModes(allModes);
        setSourceModeId(sources[0]?.id ?? "");
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Could not load modes.");
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingModes(false);
      });

    return () => {
      cancelled = true;
    };
  }, [open, targetModeId]);

  useEffect(() => {
    if (!open || !sourceModeId) return;
    let cancelled = false;
    setPreview(null);
    setSelected(new Set());
    setError(null);
    setLoadingPreview(true);

    void authoringImportApi
      .preview(sourceModeId, targetModeId)
      .then((nextPreview) => {
        if (cancelled) return;
        setPreview(nextPreview);
        setSelected(
          new Set(
            nextPreview.items
              .filter((item) => item.status === "ready")
              .map(itemKey),
          ),
        );
      })
      .catch((err) => {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : "Could not preview this import.");
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingPreview(false);
      });

    return () => {
      cancelled = true;
    };
  }, [open, sourceModeId, targetModeId]);

  const sourceModes = modes.filter((mode) => mode.id !== targetModeId);
  const targetMode = modes.find((mode) => mode.id === targetModeId);
  const grouped = useMemo(
    () =>
      GROUPS.map((group) => ({
        ...group,
        items: preview?.items.filter((item) => item.kind === group.kind) ?? [],
      })).filter((group) => group.items.length > 0),
    [preview],
  );
  const readyCount = preview?.items.filter((item) => item.status === "ready").length ?? 0;
  const conflictCount = preview?.items.length ? preview.items.length - readyCount : 0;

  if (!open) return null;

  function toggle(item: AuthoringImportItem) {
    if (item.status !== "ready") return;
    const key = itemKey(item);
    setSelected((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }

  async function commit() {
    if (!preview || selected.size === 0) return;
    setCommitting(true);
    setError(null);
    try {
      const result = await authoringImportApi.commit(
        preview.source_mode.id,
        preview.target_mode.id,
        preview.items
          .filter((item) => selected.has(itemKey(item)))
          .map((item) => ({ kind: item.kind, resource_id: item.resource_id })),
      );
      const importedCount = result.imported.length;
      const details = [`${importedCount} item${importedCount === 1 ? "" : "s"} imported`];
      if (result.skipped.length > 0) {
        details.push(`${result.skipped.length} skipped because they already exist`);
      }
      if (result.missing_track_paths.length > 0) {
        details.push(`${result.missing_track_paths.length} missing tracks omitted`);
      }
      if (result.skipped.length > 0 || result.missing_track_paths.length > 0) {
        toast.warn("Authoring import completed with skips", details.join(" · "));
      } else {
        toast.success("Authoring imported", details[0]);
      }
      onImported(result);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : "Import failed.");
      setCommitting(false);
    }
  }

  return (
    <Modal
      ariaLabel="Import authoring from another mode"
      title="Import authoring"
      className="authoring-import-modal"
      bodyClassName="authoring-import-body"
      closeButton
      onClose={onClose}
      footer={
        <>
          <button type="button" onClick={onClose} disabled={committing}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => void commit()}
            disabled={
              committing ||
              loadingModes ||
              loadingPreview ||
              preview === null ||
              selected.size === 0
            }
          >
            {committing
              ? "Importing…"
              : `Import ${selected.size} item${selected.size === 1 ? "" : "s"}`}
          </button>
        </>
      }
    >
      <p className="muted small">
        Copy selected resources into the active mode. Existing names and IDs are kept
        unchanged and never overwritten. Select related presets, playlists, and
        soundboards together when a cue or interrupt refers to them.
      </p>

      {loadingModes ? <p role="status">Loading modes…</p> : null}
      {!loadingModes && sourceModes.length === 0 ? (
        <p className="empty-detail">
          There is no other mode to import from. Create another mode first.
        </p>
      ) : null}

      {sourceModes.length > 0 ? (
        <label className="field authoring-import-source">
          <span>Source mode</span>
          <select
            data-autofocus
            value={sourceModeId}
            onChange={(event) => setSourceModeId(event.target.value)}
            disabled={committing}
          >
            {sourceModes.map((mode) => (
              <option key={mode.id} value={mode.id}>
                {mode.name}
              </option>
            ))}
          </select>
        </label>
      ) : null}

      {preview ? (
        <div className="authoring-import-route" aria-label="Import direction">
          <span className="authoring-import-mode">
            <span className="muted small">From</span>
            <strong>{preview.source_mode.name}</strong>
          </span>
          <span className="authoring-import-arrow" aria-hidden="true">
            →
          </span>
          <span className="authoring-import-mode">
            <span className="muted small">Into active mode</span>
            <strong>{preview.target_mode.name}</strong>
          </span>
        </div>
      ) : targetMode ? (
        <p className="muted small">Target: {targetMode.name}</p>
      ) : null}

      {loadingPreview ? <p role="status">Reviewing available resources…</p> : null}

      {preview && preview.items.length === 0 ? (
        <p className="empty-detail">This source mode has no authored resources to import.</p>
      ) : null}

      {preview && preview.items.length > 0 ? (
        <div className="authoring-import-review">
          <div className="authoring-import-review-summary">
            <span>
              {readyCount} available
              {conflictCount > 0 ? ` · ${conflictCount} already in target` : ""}
            </span>
            <span className="authoring-import-selection-count">
              {selected.size} selected
            </span>
          </div>
          <div className="authoring-import-groups">
            {grouped.map((group) => (
              <fieldset key={group.kind} className="authoring-import-group">
                <legend>{group.label}</legend>
                {group.items.map((item) => {
                  const conflict = item.status === "conflict";
                  return (
                    <label
                      key={itemKey(item)}
                      className={`authoring-import-item${conflict ? " is-conflict" : ""}`}
                      title={item.reason ?? undefined}
                    >
                      <input
                        type="checkbox"
                        checked={!conflict && selected.has(itemKey(item))}
                        disabled={conflict || committing}
                        onChange={() => toggle(item)}
                      />
                      <span className="authoring-import-item-copy">
                        <strong>{item.name}</strong>
                        <span className="muted small">{item.summary}</span>
                      </span>
                      {conflict ? (
                        <span className="tag authoring-import-existing">Already here</span>
                      ) : null}
                    </label>
                  );
                })}
              </fieldset>
            ))}
          </div>
        </div>
      ) : null}

      {error ? (
        <p role="alert" className="error small">
          {error}
        </p>
      ) : null}
    </Modal>
  );
}
