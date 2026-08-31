import { useCallback, useEffect, useRef, useState } from "react";
import type { ChangeEvent } from "react";

import { confirmDialog } from "@/components/confirmDialog";
import { EmptyState } from "@/components/EmptyState";
import { cleanupApi } from "@/core/api";
import type { CleanupBatchSummary } from "@/core/api";
import { toast } from "@/core/toast";

function reportRevert(reverted: number, skipped: { reason: string }[]) {
  if (skipped.length === 0) {
    toast.success(`Reverted ${reverted} change${reverted === 1 ? "" : "s"}`);
    return;
  }
  const sample = skipped
    .slice(0, 3)
    .map((item) => item.reason)
    .join("\n");
  toast.warn(
    `Reverted ${reverted}, skipped ${skipped.length}`,
    `${sample}${skipped.length > 3 ? `\n…and ${skipped.length - 3} more` : ""}`,
  );
}

async function downloadJournal(batchId: number) {
  try {
    const detail = await cleanupApi.batch(batchId);
    const blob = new Blob([JSON.stringify(detail, null, 2)], {
      type: "application/json",
    });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = `cleanup-batch-${batchId}.json`;
    anchor.click();
    URL.revokeObjectURL(url);
  } catch (error) {
    toast.error("Download failed", error instanceof Error ? error.message : undefined);
  }
}

export function CleanupHistoryPanel({ onApplied }: { onApplied?: () => void }) {
  const [batches, setBatches] = useState<CleanupBatchSummary[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [revertingId, setRevertingId] = useState<number | null>(null);
  const journalInputRef = useRef<HTMLInputElement>(null);

  const loadBatches = useCallback(async () => {
    setBatches(null);
    setLoadError(null);
    try {
      setBatches(await cleanupApi.batches());
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : "Cleanup history is unavailable.");
      setBatches([]);
    }
  }, []);

  useEffect(() => {
    void loadBatches();
  }, [loadBatches]);

  async function revertBatch(batch: CleanupBatchSummary) {
    const ok = await confirmDialog({
      title: `Revert cleanup run #${batch.id}?`,
      body:
        `${batch.item_count} change${batch.item_count === 1 ? "" : "s"} (${batch.scope_label || "no label"}) ` +
        "will be undone — renames restored, tags set back. Files changed again since are skipped, not clobbered.",
      tone: "danger",
      confirmLabel: "Revert",
    });
    if (!ok) return;
    setRevertingId(batch.id);
    try {
      const result = await cleanupApi.revertBatch(batch.id);
      reportRevert(result.reverted, result.skipped);
      onApplied?.();
      await loadBatches();
    } catch (error) {
      toast.error("Revert failed", error instanceof Error ? error.message : undefined);
    } finally {
      setRevertingId(null);
    }
  }

  function onJournalFilePick(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    void (async () => {
      let items: unknown[];
      try {
        const parsed: unknown = JSON.parse(await file.text());
        const maybe = Array.isArray(parsed) ? parsed : (parsed as { items?: unknown[] })?.items;
        if (!Array.isArray(maybe) || maybe.length === 0) {
          throw new Error("no journal items found in the file");
        }
        items = maybe;
      } catch (error) {
        toast.error(
          "Not a cleanup journal",
          error instanceof Error ? error.message : undefined,
        );
        return;
      }
      const ok = await confirmDialog({
        title: "Revert from journal file?",
        body: `${items.length} recorded change${items.length === 1 ? "" : "s"} from “${file.name}” will be undone where the files still match.`,
        tone: "danger",
        confirmLabel: "Revert",
      });
      if (!ok) return;
      try {
        const result = await cleanupApi.revertJournal(items);
        reportRevert(result.reverted, result.skipped);
        onApplied?.();
        await loadBatches();
      } catch (error) {
        toast.error("Revert failed", error instanceof Error ? error.message : undefined);
      }
    })();
  }

  return (
    <div className="cleanup-history">
      {batches === null ? <p className="muted small">Loading cleanup history…</p> : null}
      {loadError !== null ? (
        <div className="cleanup-history-error" role="alert">
          <div>
            <strong>Cleanup history could not be loaded</strong>
            <p className="muted small">{loadError}</p>
          </div>
          <button type="button" className="btn-secondary" onClick={() => void loadBatches()}>
            Retry
          </button>
        </div>
      ) : null}
      {batches !== null && loadError === null && batches.length === 0 ? (
        <EmptyState title="No cleanup runs yet">
          Applied cleanup runs appear here with their full change journal. Each can be
          downloaded as JSON or safely reverted.
        </EmptyState>
      ) : null}
      {batches?.map((batch) => (
        <div key={batch.id} className="cleanup-batch-row">
          <div className="cleanup-batch-main">
            <span>
              <strong>#{batch.id}</strong> · {new Date(batch.created_at).toLocaleString()}
            </span>
            <span className="muted small">
              {batch.item_count} change{batch.item_count === 1 ? "" : "s"}
              {batch.scope_label ? ` · ${batch.scope_label}` : ""}
            </span>
          </div>
          {batch.reverted_at !== null ? (
            <span className="badge">reverted</span>
          ) : (
            <button
              type="button"
              disabled={revertingId !== null}
              onClick={() => void revertBatch(batch)}
            >
              {revertingId === batch.id ? "Reverting…" : "Revert"}
            </button>
          )}
          <button
            type="button"
            className="btn-ghost"
            onClick={() => void downloadJournal(batch.id)}
          >
            Download
          </button>
        </div>
      ))}
      <div className="cleanup-journal-upload">
        <input
          ref={journalInputRef}
          type="file"
          accept="application/json,.json"
          className="sr-only"
          tabIndex={-1}
          onChange={onJournalFilePick}
        />
        <button
          type="button"
          className="btn-link"
          onClick={() => journalInputRef.current?.click()}
        >
          Revert from a downloaded journal file…
        </button>
      </div>
    </div>
  );
}
