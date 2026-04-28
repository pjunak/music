import { useEffect, useRef, useState } from "react";
import type { ChangeEvent, DragEvent } from "react";

import { libraryApi } from "@/core/api";
import type { IncomingFile, IngestResult } from "@/core/api";

interface UploadManagerProps {
  /** Called after a successful ingest so the caller can refresh whatever
   *  derived view it shows (e.g. the library search list). */
  onIngestComplete?: () => void;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

interface UploadProgress {
  /** 0..1 — derived from XHR upload events. Falls back to 1 when totals unavailable. */
  fraction: number;
  /** Bytes transferred so far. */
  loaded: number;
  /** Total bytes to transfer (sum across all files). */
  total: number;
  /** Number of files in the batch. */
  files: number;
}

/** Upload via XMLHttpRequest because fetch() doesn't expose upload-progress
 *  events. Same response semantics as the rest of api.ts. */
function uploadWithProgress(
  files: File[],
  onProgress: (p: UploadProgress) => void,
): Promise<{ saved: IncomingFile[] }> {
  return new Promise((resolve, reject) => {
    const form = new FormData();
    for (const f of files) form.append("files", f, f.name);
    const total = files.reduce((sum, f) => sum + f.size, 0);
    const xhr = new XMLHttpRequest();
    xhr.open("POST", "/api/library/upload");
    xhr.withCredentials = true;
    xhr.responseType = "text";
    xhr.upload.onprogress = (e) => {
      if (e.lengthComputable) {
        onProgress({
          fraction: e.total > 0 ? e.loaded / e.total : 1,
          loaded: e.loaded,
          total: e.total,
          files: files.length,
        });
      } else {
        onProgress({ fraction: 1, loaded: total, total, files: files.length });
      }
    };
    xhr.onerror = () => reject(new Error("network error during upload"));
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(JSON.parse(xhr.responseText) as { saved: IncomingFile[] });
        } catch {
          reject(new Error("upload succeeded but response wasn't JSON"));
        }
        return;
      }
      let detail = `${xhr.status}: ${xhr.statusText}`;
      try {
        const body = JSON.parse(xhr.responseText) as { detail?: unknown };
        if (body && typeof body.detail === "string") detail = body.detail;
      } catch {
        if (xhr.responseText) {
          detail = xhr.responseText.slice(0, 200);
        }
      }
      reject(new Error(`API ${xhr.status}: ${detail}`));
    };
    xhr.send(form);
  });
}

export function UploadManager({ onIngestComplete }: UploadManagerProps) {
  const [incoming, setIncoming] = useState<IncomingFile[]>([]);
  const [progress, setProgress] = useState<UploadProgress | null>(null);
  const [ingesting, setIngesting] = useState(false);
  const [autotag, setAutotag] = useState(false);
  const [lastIngest, setLastIngest] = useState<IngestResult | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  async function refreshIncoming() {
    try {
      const res = await libraryApi.listIncoming();
      setIncoming(res.files);
    } catch (e) {
      setError(e instanceof Error ? e.message : "failed to list incoming");
    }
  }

  useEffect(() => {
    void refreshIncoming();
  }, []);

  async function uploadFiles(files: File[]) {
    if (files.length === 0) return;
    setError(null);
    setProgress({ fraction: 0, loaded: 0, total: 0, files: files.length });
    try {
      await uploadWithProgress(files, setProgress);
      await refreshIncoming();
    } catch (e) {
      setError(e instanceof Error ? e.message : "upload failed");
    } finally {
      setProgress(null);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  }

  function onPickFiles(e: ChangeEvent<HTMLInputElement>) {
    const files = e.target.files ? Array.from(e.target.files) : [];
    void uploadFiles(files);
  }

  function onDrop(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    setDragOver(false);
    const files = Array.from(e.dataTransfer.files);
    void uploadFiles(files);
  }

  function onDragOver(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    setDragOver(true);
  }

  function onDragLeave(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    setDragOver(false);
  }

  async function deleteIncoming(name: string) {
    setError(null);
    try {
      await libraryApi.deleteIncoming(name);
      await refreshIncoming();
    } catch (e) {
      setError(e instanceof Error ? e.message : "delete failed");
    }
  }

  async function runIngest() {
    setIngesting(true);
    setError(null);
    try {
      const result = await libraryApi.ingest(autotag);
      setLastIngest(result);
      await refreshIncoming();
      onIngestComplete?.();
    } catch (e) {
      setError(e instanceof Error ? e.message : "ingest failed");
    } finally {
      setIngesting(false);
    }
  }

  const hasFiles = incoming.length > 0;
  const uploading = progress !== null;
  const busy = uploading || ingesting;

  return (
    <section className="upload-manager">
      <div className="upload-manager-header">
        <h3>Add music</h3>
        <div className="upload-manager-actions">
          <label className="autotag-toggle" title="Try to match each file against MusicBrainz. Files without a strong match will be skipped.">
            <input
              type="checkbox"
              checked={autotag}
              disabled={busy}
              onChange={(e) => setAutotag(e.target.checked)}
            />
            <span>Match via MusicBrainz</span>
          </label>
          <button
            type="button"
            onClick={() => void runIngest()}
            disabled={busy || !hasFiles}
          >
            {ingesting ? "Importing…" : "Run import"}
          </button>
        </div>
      </div>

      <div
        className={`drop-zone${dragOver ? " drop-zone-active" : ""}${
          uploading ? " drop-zone-uploading" : ""
        }`}
        onDrop={onDrop}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onClick={() => !busy && fileInputRef.current?.click()}
        role="button"
        tabIndex={0}
        aria-busy={uploading}
        onKeyDown={(e) => {
          if (!busy && (e.key === "Enter" || e.key === " ")) {
            fileInputRef.current?.click();
          }
        }}
      >
        {uploading && progress !== null ? (
          <div className="upload-progress">
            <div className="upload-progress-label">
              Uploading {progress.files} file{progress.files === 1 ? "" : "s"} —{" "}
              {formatSize(progress.loaded)} / {formatSize(progress.total)}
            </div>
            <progress value={progress.fraction} max={1} />
          </div>
        ) : (
          "Drop audio files here, or click to choose. They'll queue for import."
        )}
        <input
          ref={fileInputRef}
          type="file"
          multiple
          accept="audio/*"
          onChange={onPickFiles}
          hidden
        />
      </div>

      {error !== null ? <p className="error small">{error}</p> : null}

      {hasFiles ? (
        <ul className="incoming-list">
          {incoming.map((f) => (
            <li key={f.name} className="incoming-item">
              <span className="incoming-name" title={f.name}>{f.name}</span>
              <span className="muted small">{formatSize(f.size_bytes)}</span>
              <button
                type="button"
                onClick={() => void deleteIncoming(f.name)}
                disabled={busy}
                title="Discard this file without importing"
              >
                Discard
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted small">No files awaiting import.</p>
      )}

      {lastIngest !== null ? (
        <div
          className={`ingest-result ${
            lastIngest.ok ? "ingest-ok" : "ingest-failed"
          }`}
        >
          <p className="ingest-summary">
            {lastIngest.ok ? "Import completed" : `Import failed (exit code ${lastIngest.returncode})`}
            {lastIngest.imported > 0 || lastIngest.skipped > 0 ? (
              <span className="ingest-counts">
                {" — "}
                <strong>{lastIngest.imported}</strong> imported
                {lastIngest.skipped > 0 ? (
                  <>, <strong>{lastIngest.skipped}</strong> skipped</>
                ) : null}
              </span>
            ) : null}
          </p>

          {lastIngest.imported === 0 && lastIngest.skipped > 0 && !autotag ? (
            <p className="muted small">
              Beets skipped every file. With autotag off this is rare — check
              the output below; the most common cause is a missing or
              misconfigured Beets library directory.
            </p>
          ) : null}

          {lastIngest.imported === 0 && lastIngest.skipped > 0 && autotag ? (
            <p className="muted small">
              Beets couldn't auto-match these files against MusicBrainz. Try
              again with <strong>Match via MusicBrainz</strong> off — that
              imports files using their existing tags instead of querying MB.
            </p>
          ) : null}

          {lastIngest.stderr ? (
            <pre className="ingest-output ingest-stderr">{lastIngest.stderr}</pre>
          ) : null}
          {lastIngest.stdout ? (
            <pre className="ingest-output">{lastIngest.stdout}</pre>
          ) : null}

          {!lastIngest.stdout && !lastIngest.stderr ? (
            <p className="muted small">
              beet returned no output. Likely a config issue on the server —
              check that <code>directory:</code> and <code>library:</code> are
              set in <code>beets.yaml</code>.
            </p>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
