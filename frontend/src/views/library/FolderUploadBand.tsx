import { useState } from "react";
import type { ChangeEvent, DragEvent } from "react";

import { uploadConflictDialog } from "@/components/conflictDialog";
import { ImportIcon } from "@/components/icons";
import { libraryApi, sfxApi } from "@/core/api";
import type { UploadCheckItem, UploadConflict } from "@/core/api";
import {
  collectEntries,
  entriesFromItems,
  groupByParent,
  isAudioFile,
} from "@/core/dropTraversal";
import type { CollectedFile } from "@/core/dropTraversal";
import { toast } from "@/core/toast";

import { followConversion } from "./conversion";
import { collisionKey, formatSize } from "./shared";
import type { Root } from "./shared";
import { FolderHeader } from "./FolderHeader";

/** One unified band: the folder breadcrumb on the left, a compact Upload
 *  affordance on the right — and the WHOLE band is the drop target (drag
 *  files anywhere over it). Replaces the old two stacked full-width rows
 *  (breadcrumb row + separate drop-zone row). */
export function FolderBand({
  root,
  dest,
  onPathSelect,
  onUploaded,
  onNavigate,
}: {
  root: Root;
  dest: string;
  onPathSelect: (p: string) => void;
  onUploaded: () => void;
  /** Navigate the browser to a folder path. Used after a folder drop so
   *  the operator lands inside the folder they just uploaded — otherwise
   *  the files vanish into a collapsed subfolder and it looks like the
   *  upload did nothing. */
  onNavigate: (path: string) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<{ loaded: number; total: number } | null>(
    null,
  );
  const [dragOver, setDragOver] = useState(false);

  /** Build a single destination path under MUSIC_DIR / SFX_LIBRARY_DIR by
   *  joining the currently-selected folder with a relative sub-path that
   *  came from a dropped folder's structure. Empty sub-path means the
   *  drop's destination is just the current dest unchanged. */
  function joinDest(subPath: string): string {
    if (subPath === "") return dest;
    return dest === "" ? subPath : `${dest}/${subPath}`;
  }

  /** Run uploads for a list of collected files. Files at the same relative
   *  parent dir are uploaded together (single multipart request per dir);
   *  multiple parent dirs are uploaded sequentially so the progress bar
   *  reads as one monotonic operation across the whole drop.
   *
   *  Note: parallel uploads would be faster but make progress reporting
   *  fiddly and add server load. For a single-user home server with an
   *  occasional album drop, sequential is the right trade. */
  async function sendCollected(collected: CollectedFile[]) {
    if (collected.length === 0) {
      toast.warn(
        "Nothing to upload",
        "Drop contained no audio files (we accept mp3, flac, ogg, opus, m4a, aac, wav, wma).",
      );
      return;
    }
    setBusy(true);
    let savedTotal = 0;
    let skippedCount = 0;
    const conversionJobs: string[] = [];
    try {
      const groups = groupByParent(collected);

      // Preflight: does anything already exist at the target paths? Ask the
      // operator how to handle it BEFORE sending bytes — so re-uploading
      // after a half-failed upload doesn't silently mint "-1" copies.
      const checkItems: UploadCheckItem[] = [];
      for (const [subDir, files] of groups)
        for (const f of files)
          checkItems.push({ dest: joinDest(subDir), name: f.name });

      let conflict: UploadConflict | undefined;
      const collisionKeys = new Set<string>();
      try {
        const { collisions } =
          root === "music"
            ? await libraryApi.checkUpload(checkItems)
            : await sfxApi.checkUpload(checkItems);
        if (collisions.length > 0) {
          const choice = await uploadConflictDialog({
            count: collisions.length,
            total: checkItems.length,
            sampleNames: collisions.slice(0, 5).map((c) => c.name),
          });
          if (choice === null) return; // cancelled — abort the whole drop
          conflict = choice;
          for (const c of collisions) collisionKeys.add(collisionKey(c.dest, c.name));
        }
      } catch {
        // Preflight failed (rare — same server). Fall through to a normal
        // upload; the backend default keeps both, and any genuine failure
        // surfaces via the upload's own error handling below.
      }

      // For "skip", drop colliding files client-side so we don't waste
      // bandwidth re-sending bytes the server would discard. conflict=skip
      // still goes to the backend to cover a file that appears between the
      // check and the upload (race-safety).
      let workGroups: Array<[string, File[]]> = [...groups];
      if (conflict === "skip") {
        workGroups = workGroups
          .map(([subDir, files]): [string, File[]] => {
            const groupDest = joinDest(subDir);
            const kept = files.filter(
              (f) => !collisionKeys.has(collisionKey(groupDest, f.name)),
            );
            skippedCount += files.length - kept.length;
            return [subDir, kept];
          })
          .filter(([, files]) => files.length > 0);
      }

      if (workGroups.length === 0) {
        // Everything collided and the operator chose to skip — nothing left.
        toast.info(
          "Nothing to upload",
          `All ${skippedCount} file${skippedCount === 1 ? "" : "s"} already exist.`,
        );
        return;
      }

      const totalBytes = workGroups.reduce(
        (sum, [, files]) => sum + files.reduce((s, f) => s + f.size, 0),
        0,
      );
      setProgress({ loaded: 0, total: totalBytes });

      // First top-level subfolder this drop lands files in — used to navigate
      // the operator into it afterwards (computed post-skip so we never jump
      // into a folder that ended up empty). Null = loose files only, stay put.
      let firstSubfolder: string | null = null;
      for (const [subDir] of workGroups) {
        if (subDir !== "") {
          firstSubfolder = subDir.split("/")[0] ?? null;
          break;
        }
      }

      let baseLoaded = 0;
      for (const [subDir, files] of workGroups) {
        const groupDest = joinDest(subDir);
        const groupBytes = files.reduce((s, f) => s + f.size, 0);
        // upload reports (loaded) for that one group; re-base against the
        // cumulative drop total so the bar moves monotonically.
        const onGroupProgress = (loaded: number) => {
          setProgress({ loaded: baseLoaded + loaded, total: totalBytes });
        };
        const r =
          root === "music"
            ? await libraryApi.upload(files, groupDest, onGroupProgress, conflict)
            : await sfxApi.upload(files, groupDest, onGroupProgress, conflict);
        savedTotal += r.saved.length;
        skippedCount += r.skipped.length;
        baseLoaded += groupBytes;
        if ("conversion_job_id" in r && r.conversion_job_id) {
          conversionJobs.push(r.conversion_job_id);
        }
      }

      const folderCount = workGroups.length;
      const where =
        folderCount > 1
          ? `across ${folderCount} folders under "${dest || "(root)"}"`
          : `→ ${dest || "(root)"}`;
      const detail =
        skippedCount > 0 ? `${where} · skipped ${skippedCount} existing` : where;
      toast.success(
        `Uploaded ${savedTotal}`,
        conversionJobs.length > 0 ? `${detail} · converting in the background` : detail,
      );
      for (const jobId of conversionJobs) void followConversion(jobId, onUploaded);
      // Refresh (re-reads the index/tree so new folders + tracks show up).
      // For a folder drop, navigate INTO the uploaded folder so the result
      // is visible instead of hiding in a collapsed subtree. Loose-file
      // drops stay put; the files appear in the current track table.
      onUploaded();
      if (firstSubfolder !== null) {
        onNavigate(joinDest(firstSubfolder));
      }
    } catch (e) {
      // Partial uploads happen — some files may have landed before the
      // failing request. Tell the operator how many made it so they know
      // whether to retry from scratch or just re-drop the missing folder.
      toast.error(
        "Upload failed",
        `${savedTotal} saved before error: ${e instanceof Error ? e.message : "unknown"}`,
      );
    } finally {
      setBusy(false);
      setProgress(null);
    }
  }

  // NOTE: this handler is intentionally NOT async. The DataTransfer and
  // the FileSystemEntry objects it yields are only valid during the
  // synchronous part of the drop event — the moment we `await`, the
  // browser (Firefox especially) neuters them and directory reads come
  // back empty. So we capture both the entry handles and the flat file
  // list synchronously here, then hand them to the async walker.
  function onDrop(e: DragEvent<HTMLDivElement>) {
    e.preventDefault();
    setDragOver(false);
    const entries = entriesFromItems(e.dataTransfer.items);
    const flatFiles = Array.from(e.dataTransfer.files);
    void processDrop(entries, flatFiles);
  }

  async function processDrop(entries: FileSystemEntry[], flatFiles: File[]) {
    // Prefer the entry walk (handles folders). Fall back to the flat file
    // list — captured synchronously above — when the entry API gave us
    // nothing (older browsers, programmatic drops, or a drop that somehow
    // produced no usable entries).
    let collected: CollectedFile[] = [];
    if (entries.length > 0) {
      try {
        collected = await collectEntries(entries);
      } catch {
        collected = [];
      }
    }
    if (collected.length === 0) {
      collected = flatFiles
        .filter((f) => isAudioFile(f.name))
        .map((file) => ({ relativePath: file.name, file }));
    }
    await sendCollected(collected);
  }
  function onPick(e: ChangeEvent<HTMLInputElement>) {
    // The file picker honours accept="audio/*" as a hint but the user can
    // still flip to "all files" — filter here so non-audio picks aren't
    // silently sent and stored as unindexed clutter.
    const collected: CollectedFile[] = Array.from(e.target.files ?? [])
      .filter((f) => isAudioFile(f.name))
      .map((file) => ({ relativePath: file.name, file }));
    e.target.value = "";
    void sendCollected(collected);
  }

  return (
    <div
      className={`folder-band${dragOver ? " folder-band-drop" : ""}${
        busy ? " folder-band-busy" : ""
      }`}
      onDragOver={(e) => {
        e.preventDefault();
        setDragOver(true);
      }}
      onDragLeave={(e) => {
        e.preventDefault();
        setDragOver(false);
      }}
      onDrop={onDrop}
    >
      <FolderHeader path={dest} root={root} onPathSelect={onPathSelect} />
      {busy && progress !== null ? (
        <div className="upload-progress folder-band-progress">
          <div className="upload-progress-label">
            Uploading — {formatSize(progress.loaded)} / {formatSize(progress.total)}
          </div>
          <progress
            aria-label="Upload progress"
            value={progress.total > 0 ? progress.loaded / progress.total : 0}
            max={1}
          />
        </div>
      ) : dragOver ? (
        <span className="folder-band-hint">
          Drop to upload to <strong>{dest === "" ? "(root)" : dest}</strong>
        </span>
      ) : (
        <label
          className="folder-band-upload"
          title={`Upload audio to ${dest === "" ? "(root)" : dest}`}
        >
          <input type="file" multiple accept="audio/*" onChange={onPick} hidden />
          <ImportIcon aria-hidden="true" />
          <span>Upload</span>
        </label>
      )}
    </div>
  );
}
