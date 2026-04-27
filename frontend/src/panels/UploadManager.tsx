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

export function UploadManager({ onIngestComplete }: UploadManagerProps) {
  const [incoming, setIncoming] = useState<IncomingFile[]>([]);
  const [uploading, setUploading] = useState(false);
  const [ingesting, setIngesting] = useState(false);
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
    setUploading(true);
    setError(null);
    try {
      await libraryApi.upload(files);
      await refreshIncoming();
    } catch (e) {
      setError(e instanceof Error ? e.message : "upload failed");
    } finally {
      setUploading(false);
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
      const result = await libraryApi.ingest();
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
  const busy = uploading || ingesting;

  return (
    <section className="upload-manager">
      <div className="upload-manager-header">
        <h3>Add music</h3>
        <button
          type="button"
          onClick={() => void runIngest()}
          disabled={busy || !hasFiles}
        >
          {ingesting ? "Importing…" : "Run import"}
        </button>
      </div>

      <div
        className={`drop-zone${dragOver ? " drop-zone-active" : ""}`}
        onDrop={onDrop}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onClick={() => fileInputRef.current?.click()}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") fileInputRef.current?.click();
        }}
      >
        {uploading
          ? "Uploading…"
          : "Drop audio files here, or click to choose. They'll be queued for import."}
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
              <span className="incoming-name">{f.name}</span>
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
        <details className="ingest-result">
          <summary>
            Last import: {lastIngest.ok ? "ok" : `failed (${lastIngest.returncode})`}
          </summary>
          {lastIngest.stdout ? (
            <pre className="ingest-output">{lastIngest.stdout}</pre>
          ) : null}
          {lastIngest.stderr ? (
            <pre className="ingest-output ingest-stderr">{lastIngest.stderr}</pre>
          ) : null}
        </details>
      ) : null}
    </section>
  );
}
