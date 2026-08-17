import { useEffect, useMemo, useRef, useState } from "react";

import { Modal } from "@/components/Modal";
import { authoringImportApi, modesApi } from "@/core/api";
import type {
  AuthoringImportItem,
  AuthoringImportKind,
  AuthoringImportPreview,
  AuthoringImportResult,
  AuthoringImportSelection,
} from "@/core/api";
import { toast } from "@/core/toast";
import type { ModeSummary } from "@/core/types";

const JSON_FILE_MAX_BYTES = 1024 * 1024;

const GROUPS: { kind: AuthoringImportKind; label: string }[] = [
  { kind: "playlist", label: "Playlists" },
  { kind: "soundboard", label: "Soundboards" },
  { kind: "interrupt", label: "Interrupts" },
  { kind: "preset", label: "EQ presets" },
  { kind: "cue", label: "Cues" },
];

type ImportSourceKind = "mode" | "file" | "paste";

function itemKey(item: Pick<AuthoringImportItem, "kind" | "resource_id">) {
  return `${item.kind}:${item.resource_id}`;
}

function parseDocumentText(text: string): unknown {
  if (new TextEncoder().encode(text).byteLength > JSON_FILE_MAX_BYTES) {
    throw new Error("The JSON document is larger than the 1 MiB import limit.");
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(text);
  } catch {
    throw new Error("This is not valid JSON. Check the document and try again.");
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("The import document must be one JSON object.");
  }
  return parsed;
}

function errorMessage(error: unknown, fallback: string) {
  return error instanceof Error ? error.message : fallback;
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
  const previewRequest = useRef(0);
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const [sourceKind, setSourceKind] = useState<ImportSourceKind>("mode");
  const [sourceModeId, setSourceModeId] = useState("");
  const [document, setDocument] = useState<unknown | null>(null);
  const [documentSourceName, setDocumentSourceName] = useState<string>();
  const [fileName, setFileName] = useState("");
  const [pasteText, setPasteText] = useState("");
  const [draggingFile, setDraggingFile] = useState(false);
  const [preview, setPreview] = useState<AuthoringImportPreview | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [loadingModes, setLoadingModes] = useState(false);
  const [loadingPreview, setLoadingPreview] = useState(false);
  const [committing, setCommitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    previewRequest.current += 1;
    setModes([]);
    setSourceKind("mode");
    setSourceModeId("");
    setDocument(null);
    setDocumentSourceName(undefined);
    setFileName("");
    setPasteText("");
    setDraggingFile(false);
    setPreview(null);
    setSelected(new Set());
    setError(null);
    setLoadingModes(true);
    setLoadingPreview(false);
    setCommitting(false);

    void modesApi
      .list()
      .then((allModes) => {
        if (cancelled) return;
        const sources = allModes.filter((mode) => mode.id !== targetModeId);
        setModes(allModes);
        setSourceModeId(sources[0]?.id ?? "");
      })
      .catch((requestError) => {
        if (!cancelled) {
          setError(errorMessage(requestError, "Could not load modes."));
        }
      })
      .finally(() => {
        if (!cancelled) setLoadingModes(false);
      });

    return () => {
      cancelled = true;
      previewRequest.current += 1;
    };
  }, [open, targetModeId]);

  useEffect(() => {
    if (!open || sourceKind !== "mode" || !sourceModeId) return;
    const requestId = ++previewRequest.current;
    setPreview(null);
    setSelected(new Set());
    setError(null);
    setLoadingPreview(true);

    void authoringImportApi
      .previewMode(sourceModeId, targetModeId)
      .then((nextPreview) => {
        if (requestId !== previewRequest.current) return;
        setPreview(nextPreview);
        setSelected(
          new Set(
            nextPreview.items
              .filter((item) => item.status === "ready")
              .map(itemKey),
          ),
        );
      })
      .catch((requestError) => {
        if (requestId === previewRequest.current) {
          setError(errorMessage(requestError, "Could not preview this import."));
        }
      })
      .finally(() => {
        if (requestId === previewRequest.current) setLoadingPreview(false);
      });
  }, [open, sourceKind, sourceModeId, targetModeId]);

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
  const readyCount =
    preview?.items.filter((item) => item.status === "ready").length ?? 0;
  const conflictCount =
    preview?.items.filter((item) => item.status === "conflict").length ?? 0;
  const invalidCount =
    preview?.items.filter((item) => item.status === "invalid").length ?? 0;
  const missingDependencies = useMemo(() => {
    if (!preview) return [];
    const problems: { item: AuthoringImportItem; dependency: AuthoringImportSelection }[] = [];
    for (const item of preview.items) {
      if (!selected.has(itemKey(item))) continue;
      for (const issue of item.issues) {
        if (
          issue.code === "dependency_selection_required" &&
          issue.related_item &&
          !selected.has(itemKey(issue.related_item))
        ) {
          problems.push({ item, dependency: issue.related_item });
        }
      }
    }
    return problems;
  }, [preview, selected]);

  if (!open) return null;

  function resetReview(nextSource: ImportSourceKind) {
    previewRequest.current += 1;
    setSourceKind(nextSource);
    setPreview(null);
    setSelected(new Set());
    setDocument(null);
    setDocumentSourceName(undefined);
    setFileName("");
    setError(null);
    setLoadingPreview(false);
  }

  async function reviewDocument(nextDocument: unknown, sourceName: string) {
    const requestId = ++previewRequest.current;
    setDocument(nextDocument);
    setDocumentSourceName(sourceName);
    setPreview(null);
    setSelected(new Set());
    setError(null);
    setLoadingPreview(true);
    try {
      const nextPreview = await authoringImportApi.previewDocument(
        targetModeId,
        nextDocument,
        sourceName,
      );
      if (requestId !== previewRequest.current) return;
      setPreview(nextPreview);
      setSelected(
        new Set(
          nextPreview.items
            .filter((item) => item.status === "ready")
            .map(itemKey),
        ),
      );
    } catch (requestError) {
      if (requestId === previewRequest.current) {
        setError(errorMessage(requestError, "Could not review this JSON document."));
      }
    } finally {
      if (requestId === previewRequest.current) setLoadingPreview(false);
    }
  }

  async function readFile(file: File) {
    const fileReadRequest = ++previewRequest.current;
    setLoadingPreview(false);
    setFileName(file.name);
    setError(null);
    if (!file.name.toLowerCase().endsWith(".json")) {
      setPreview(null);
      setSelected(new Set());
      setError("Choose a .json Authoring import document.");
      return;
    }
    if (file.size > JSON_FILE_MAX_BYTES) {
      setPreview(null);
      setSelected(new Set());
      setError("The JSON document is larger than the 1 MiB import limit.");
      return;
    }
    try {
      const text = await file.text();
      if (fileReadRequest !== previewRequest.current) return;
      const nextDocument = parseDocumentText(text);
      await reviewDocument(nextDocument, file.name);
    } catch (readError) {
      if (fileReadRequest !== previewRequest.current) return;
      setPreview(null);
      setSelected(new Set());
      setError(errorMessage(readError, "Could not read this JSON document."));
    }
  }

  async function reviewPaste() {
    try {
      await reviewDocument(parseDocumentText(pasteText), "Pasted JSON");
    } catch (parseError) {
      setPreview(null);
      setSelected(new Set());
      setError(errorMessage(parseError, "Could not read the pasted JSON."));
    }
  }

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
    if (
      !preview ||
      selected.size === 0 ||
      missingDependencies.length > 0
    ) {
      return;
    }
    if (preview.source.type === "document" && document === null) {
      setError("Review the JSON document again before importing.");
      return;
    }
    const selections = preview.items
      .filter((item) => selected.has(itemKey(item)))
      .map((item) => ({ kind: item.kind, resource_id: item.resource_id }));
    setCommitting(true);
    setError(null);
    try {
      const result =
        preview.source.type === "mode"
          ? await authoringImportApi.commitMode(
              preview.source.id,
              preview.target_mode.id,
              selections,
            )
          : await authoringImportApi.commitDocument(
              preview.target_mode.id,
              document,
              selections,
              documentSourceName,
            );
      const importedCount = result.imported.length;
      const details = [
        `${importedCount} item${importedCount === 1 ? "" : "s"} imported`,
      ];
      if (result.skipped.length > 0) {
        details.push(`${result.skipped.length} skipped after the final review`);
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
    } catch (commitError) {
      setError(errorMessage(commitError, "Import failed."));
      setCommitting(false);
    }
  }

  const hasNoModeSource = !loadingModes && sourceModes.length === 0;
  const sourceEmptyLabel =
    preview?.source.type === "mode"
      ? "This source mode has no authored resources to import."
      : "This document has no authored resources to import.";

  return (
    <Modal
      ariaLabel="Import authoring"
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
              loadingPreview ||
              preview === null ||
              selected.size === 0 ||
              missingDependencies.length > 0
            }
          >
            {committing
              ? "Importing…"
              : missingDependencies.length > 0
                ? "Select required items"
                : `Import ${selected.size} item${selected.size === 1 ? "" : "s"}`}
          </button>
        </>
      }
    >
      <p className="muted small">
        Bring in selected resources without overwriting anything already in the active
        mode. Every document is checked on the server before it can be imported.
      </p>

      <fieldset className="authoring-import-source-picker">
        <legend>Choose a source</legend>
        <div className="authoring-import-source-options">
          <label className={sourceKind === "mode" ? "is-active" : undefined}>
            <input
              data-autofocus
              type="radio"
              name="authoring-import-source"
              value="mode"
              checked={sourceKind === "mode"}
              disabled={committing}
              onChange={() => resetReview("mode")}
            />
            <span>
              <strong>Another mode</strong>
              <small>Copy existing authored resources</small>
            </span>
          </label>
          <label className={sourceKind === "file" ? "is-active" : undefined}>
            <input
              type="radio"
              name="authoring-import-source"
              value="file"
              checked={sourceKind === "file"}
              disabled={committing}
              onChange={() => resetReview("file")}
            />
            <span>
              <strong>JSON file</strong>
              <small>Review a prepared import document</small>
            </span>
          </label>
          <label className={sourceKind === "paste" ? "is-active" : undefined}>
            <input
              type="radio"
              name="authoring-import-source"
              value="paste"
              checked={sourceKind === "paste"}
              disabled={committing}
              onChange={() => resetReview("paste")}
            />
            <span>
              <strong>Paste JSON</strong>
              <small>Use output from an assistant or tool</small>
            </span>
          </label>
        </div>
      </fieldset>

      {sourceKind === "mode" ? (
        <div className="authoring-import-source-panel">
          {loadingModes ? <p role="status">Loading modes…</p> : null}
          {hasNoModeSource ? (
            <p className="empty-detail">
              There is no other mode to copy from. You can still import a JSON file or
              paste JSON above.
            </p>
          ) : null}
          {sourceModes.length > 0 ? (
            <label className="field authoring-import-mode-select">
              <span>Source mode</span>
              <select
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
        </div>
      ) : null}

      {sourceKind === "file" ? (
        <div
          className={`authoring-import-file${draggingFile ? " is-dragging" : ""}`}
          onDragEnter={(event) => {
            event.preventDefault();
            setDraggingFile(true);
          }}
          onDragOver={(event) => event.preventDefault()}
          onDragLeave={() => setDraggingFile(false)}
          onDrop={(event) => {
            event.preventDefault();
            setDraggingFile(false);
            const [file] = Array.from(event.dataTransfer.files);
            if (file) void readFile(file);
          }}
        >
          <label className="field">
            <span>Authoring JSON document</span>
            <input
              type="file"
              accept=".json,application/json"
              disabled={committing}
              onChange={(event) => {
                const file = event.target.files?.[0];
                if (file) void readFile(file);
                event.currentTarget.value = "";
              }}
            />
          </label>
          <span className="muted small">
            {fileName || "Choose or drop a .json file · maximum 1 MiB"}
          </span>
        </div>
      ) : null}

      {sourceKind === "paste" ? (
        <div className="authoring-import-paste">
          <label className="field">
            <span>Authoring JSON</span>
            <textarea
              rows={8}
              value={pasteText}
              placeholder={'{\n  "schema": "authoring-import/v1",\n  "playlists": []\n}'}
              spellCheck={false}
              disabled={committing}
              onChange={(event) => {
                previewRequest.current += 1;
                setPasteText(event.target.value);
                setPreview(null);
                setSelected(new Set());
                setDocument(null);
                setError(null);
                setLoadingPreview(false);
              }}
            />
          </label>
          <button
            type="button"
            onClick={() => void reviewPaste()}
            disabled={!pasteText.trim() || loadingPreview || committing}
          >
            Review JSON
          </button>
        </div>
      ) : null}

      {preview ? (
        <div className="authoring-import-route" aria-label="Import direction">
          <span className="authoring-import-mode">
            <span className="muted small">From</span>
            <strong>{preview.source.name}</strong>
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

      {loadingPreview ? <p role="status">Checking resources and references…</p> : null}

      {preview && preview.items.length === 0 ? (
        <p className="empty-detail">{sourceEmptyLabel}</p>
      ) : null}

      {preview && preview.items.length > 0 ? (
        <div className="authoring-import-review">
          <div className="authoring-import-review-summary">
            <span>
              {readyCount} ready
              {conflictCount > 0 ? ` · ${conflictCount} already here` : ""}
              {invalidCount > 0 ? ` · ${invalidCount} need fixes` : ""}
            </span>
            <span className="authoring-import-selection-count">
              {selected.size} selected
            </span>
          </div>

          {missingDependencies.length > 0 ? (
            <p className="authoring-import-dependency-alert" role="alert">
              {missingDependencies.length} required related item
              {missingDependencies.length === 1 ? " is" : "s are"} not selected.
            </p>
          ) : null}

          <div className="authoring-import-groups">
            {grouped.map((group) => (
              <fieldset key={group.kind} className="authoring-import-group">
                <legend>{group.label}</legend>
                {group.items.map((item) => {
                  const blocked = item.status !== "ready";
                  const tag =
                    item.status === "conflict"
                      ? "Already here"
                      : item.status === "invalid"
                        ? "Needs fixes"
                        : null;
                  return (
                    <label
                      key={itemKey(item)}
                      className={`authoring-import-item authoring-import-item--${item.status}`}
                      title={item.reason ?? undefined}
                    >
                      <input
                        type="checkbox"
                        checked={!blocked && selected.has(itemKey(item))}
                        disabled={blocked || committing}
                        onChange={() => toggle(item)}
                      />
                      <span className="authoring-import-item-copy">
                        <strong>{item.name}</strong>
                        <span className="muted small">{item.summary}</span>
                        {item.issues.length > 0 ? (
                          <span className="authoring-import-issues">
                            {item.issues.map((issue, index) => (
                              <span
                                key={`${issue.code}:${index}`}
                                className={`authoring-import-issue is-${issue.severity}`}
                              >
                                {issue.message}
                              </span>
                            ))}
                          </span>
                        ) : null}
                      </span>
                      {tag ? (
                        <span
                          className={`tag authoring-import-status is-${item.status}`}
                        >
                          {tag}
                        </span>
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
