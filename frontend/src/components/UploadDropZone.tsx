import { useEffect, useRef, useState } from "react";
import type { ChangeEvent, DragEvent } from "react";

import { libraryApi } from "@/core/api";
import { toast } from "@/core/toast";
import type { Track } from "@/core/types";

interface Props {
  /** Default destination subfolder (preselected in the input). Empty = root. */
  defaultDest?: string;
  /** Called after each successful upload batch with the indexed tracks. */
  onUploaded?: (tracks: Track[], destination: string) => void;
}

interface ProgressState {
  loaded: number;
  total: number;
  files: number;
}

function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

export function UploadDropZone({ defaultDest = "Uploads", onUploaded }: Props) {
  const [destination, setDestination] = useState(defaultDest);
  const [folderOptions, setFolderOptions] = useState<string[]>([]);
  const [progress, setProgress] = useState<ProgressState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const fileInputRef = useRef<HTMLInputElement | null>(null);

  // Mirror the parent's preference (e.g. "current folder") into our local
  // destination state when it changes.
  useEffect(() => {
    setDestination(defaultDest);
  }, [defaultDest]);

  // Build a flat list of all known folder paths to populate the datalist.
  useEffect(() => {
    let cancelled = false;
    async function walk(path: string, acc: string[]) {
      try {
        const res = await libraryApi.tree(path);
        for (const f of res.folders) {
          acc.push(f.path);
          await walk(f.path, acc);
        }
      } catch {
        /* ignore — empty library is fine */
      }
    }
    const collected: string[] = [];
    void walk("", collected).then(() => {
      if (cancelled) return;
      const set = new Set(collected);
      set.add("Uploads");
      setFolderOptions(Array.from(set).sort());
    });
    return () => {
      cancelled = true;
    };
  }, []);

  async function uploadFiles(files: File[]) {
    if (files.length === 0) return;
    setError(null);
    setProgress({ loaded: 0, total: 0, files: files.length });
    try {
      const result = await libraryApi.upload(files, destination, (loaded, total) => {
        setProgress({ loaded, total, files: files.length });
      });
      onUploaded?.(result.saved, result.destination);
      const n = result.saved.length;
      toast.success(
        `Uploaded ${n} file${n === 1 ? "" : "s"}`,
        `Indexed under ${result.destination || "(root)"}/`,
      );
    } catch (e) {
      const detail = e instanceof Error ? e.message : "upload failed";
      setError(detail);
      toast.error("Upload failed", detail);
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

  const uploading = progress !== null;

  return (
    <div className="upload-dropzone">
      <label className="dest-picker">
        <span>Upload to</span>
        <input
          list="upload-dest-options"
          value={destination}
          onChange={(e) => setDestination(e.target.value)}
          placeholder="Uploads"
          disabled={uploading}
        />
        <datalist id="upload-dest-options">
          {folderOptions.map((f) => (
            <option key={f} value={f} />
          ))}
        </datalist>
      </label>

      <div
        className={`drop-zone${dragOver ? " drop-zone-active" : ""}${
          uploading ? " drop-zone-uploading" : ""
        }`}
        onDrop={onDrop}
        onDragOver={onDragOver}
        onDragLeave={onDragLeave}
        onClick={() => !uploading && fileInputRef.current?.click()}
        role="button"
        tabIndex={0}
        aria-busy={uploading}
        onKeyDown={(e) => {
          if (!uploading && (e.key === "Enter" || e.key === " ")) {
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
            <progress
              value={progress.total > 0 ? progress.loaded / progress.total : 0}
              max={1}
            />
          </div>
        ) : (
          <span>
            Drop audio files here, or click to choose. Lands in{" "}
            <code>{destination || "(root)"}/</code> and indexes automatically.
          </span>
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
    </div>
  );
}
