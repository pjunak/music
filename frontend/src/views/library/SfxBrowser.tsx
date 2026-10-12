import { useCallback, useEffect, useState } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { IconButton } from "@/components/IconButton";
import { PlayIcon, TrashIcon } from "@/components/icons";
import { sfxApi } from "@/core/api";
import type { SfxFile } from "@/core/api";
import { toast } from "@/core/toast";

import { formatSize, sfxAllFolders } from "./shared";
import { LibraryShell } from "./LibraryShell";

export function SfxBrowser({
  path,
  onPathChange,
  refreshKey,
  onRefresh,
}: {
  path: string;
  onPathChange: (p: string) => void;
  refreshKey: number;
  onRefresh: () => void;
}) {
  const [files, setFiles] = useState<SfxFile[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setError(null);
    void sfxApi
      .tree(path)
      .then((r) => {
        if (!cancelled) setFiles(r.files);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setError(e instanceof Error ? e.message : "load failed");
          setFiles([]);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [path, refreshKey]);

  const loadAll = useCallback(() => sfxAllFolders(), []);

  async function onDropOnFolder(folderPath: string, payload: unknown) {
    if (
      !payload ||
      typeof payload !== "object" ||
      (payload as { kind?: unknown }).kind !== "sfx-file"
    )
      return;
    const filePath = (payload as { path?: string }).path;
    if (typeof filePath !== "string") return;
    try {
      await sfxApi.moveFile(filePath, folderPath);
      toast.success("SFX moved");
      onRefresh();
    } catch (e) {
      toast.error("Move failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function onDelete(file: SfxFile) {
    const ok = await confirmDialog({
      title: "Delete SFX file?",
      body: `${file.path} will be removed from disk.`,
      tone: "danger",
      confirmLabel: "Delete",
    });
    if (!ok) return;
    try {
      await sfxApi.deleteFile(file.path);
      toast.success("Deleted", file.path);
      onRefresh();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  function preview(file: SfxFile) {
    if (!file.referenced) {
      toast.warn(
        "Preview unavailable",
        "Server only streams SFX referenced by a soundboard. Add it to a soundboard first.",
      );
      return;
    }
    const url = sfxApi.fileUrl(file.path);
    new Audio(url).play().catch(() => {
      toast.error("Preview failed", "Browser refused to play the file.");
    });
  }

  return (
    <LibraryShell
      rootKind="sfx"
      selectedPath={path}
      onPathChange={onPathChange}
      refreshKey={refreshKey}
      loadAll={loadAll}
      onDropOnFolder={(folderPath, payload) => void onDropOnFolder(folderPath, payload)}
      onRefresh={onRefresh}
      onPathReset={() => onPathChange("")}
      error={error}
    >
      {files.length === 0 ? (
          <EmptyState title="No SFX files in this folder">
            Drop audio files into the zone above, or pick a different folder
            from the tree on the left.
          </EmptyState>
        ) : (
          <ul className="sfx-file-list">
            {files.map((f) => (
              /* oxlint-disable-next-line jsx-a11y/no-noninteractive-element-interactions -- Native drag behavior belongs to the list row while its action buttons remain keyboard accessible. */
              <li
                key={f.path}
                className="sfx-file-row"
                draggable
                onDragStart={(e) => {
                  e.dataTransfer.setData(
                    "application/json",
                    JSON.stringify({ kind: "sfx-file", path: f.path }),
                  );
                  e.dataTransfer.effectAllowed = "move";
                }}
              >
                <span className="sfx-file-name">{f.name}</span>
                {f.referenced ? (
                  <span className="badge badge-ok">referenced</span>
                ) : (
                  <span className="muted small">unreferenced</span>
                )}
                <span className="muted small">{formatSize(f.size_bytes)}</span>
                <div className="sfx-file-actions">
                  <IconButton
                    label={
                      f.referenced
                        ? "Preview SFX"
                        : "Preview unavailable — add to a soundboard first"
                    }
                    icon={<PlayIcon />}
                    disabled={!f.referenced}
                    onClick={() => preview(f)}
                  />
                  <IconButton
                    label="Delete SFX file"
                    icon={<TrashIcon />}
                    variant="danger"
                    onClick={() => void onDelete(f)}
                  />
                </div>
              </li>
            ))}
          </ul>
        )}
    </LibraryShell>
  );
}

// --- shared sub-components ----------------------------------------------
