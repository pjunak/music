import { Suspense, useEffect, useState } from "react";
import { useNavigate } from "react-router-dom";

import { confirmDialog } from "@/components/confirmDialog";
import { IconButton } from "@/components/IconButton";
import {
  CompressIcon,
  LightningIcon,
  MusicNoteIcon,
  RescanIcon,
  SearchIcon,
  SparkleIcon,
  TagIcon,
  XIcon,
} from "@/components/icons";
import { libraryApi } from "@/core/api";
import { toast } from "@/core/toast";
import { useDebouncedValue } from "@/core/useDebouncedValue";
import { lazyNamed } from "@/core/lazyNamed";
import { RouteSpinner } from "@/shell/routeGuards";

import type { Root } from "./library/shared";
import { followConversion } from "./library/conversion";
import { MusicWorkspace } from "./library/MusicWorkspace";
import { SfxBrowser } from "./library/SfxBrowser";

const MoodTaggingDialog = lazyNamed(
  () => import("@/components/MoodTaggingDialog"),
  (module) => module.MoodTaggingDialog,
);

export function LibraryView() {
  const navigate = useNavigate();
  const [root, setRoot] = useState<Root>("music");
  const [path, setPath] = useState("");
  const [refreshKey, setRefreshKey] = useState(0);
  const [pendingQuery, setPendingQuery] = useState("");
  const [query, setQuery] = useState("");
  // Ticked-checkbox selection of the music list. Lives here (not in
  // MusicWorkspace) so the toolbar's cleanup shortcut can pass
  // "selected tracks" as a scope.
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [moodTaggingOpen, setMoodTaggingOpen] = useState(false);

  // Search-as-you-type: debounce keystrokes and auto-submit the result.
  // The input is always visible in the toolbar (music mode only), so the
  // user never has to click "search" first. A non-empty `query` switches
  // the body from folder-browser to search results; clearing the input
  // drops back to the folder browser at the previously-selected path.
  const debouncedPending = useDebouncedValue(pendingQuery, 250);
  useEffect(() => {
    if (debouncedPending !== query) setQuery(debouncedPending);
  }, [debouncedPending, query]);

  function clearSearch() {
    setPendingQuery("");
    setQuery("");
  }

  function selectRoot(next: Root) {
    setRoot(next);
    setPath("");
    clearSearch();
    setChecked(new Set());
  }

  return (
    <div className="library-view">
      <header className="library-toolbar">
        <div className="segmented" role="tablist" aria-label="Library root">
          <button
            type="button"
            role="tab"
            aria-selected={root === "music"}
            className="segmented-item"
            onClick={() => selectRoot("music")}
          >
            <MusicNoteIcon aria-hidden="true" /> Music
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={root === "sfx"}
            className="segmented-item"
            onClick={() => selectRoot("sfx")}
          >
            <LightningIcon aria-hidden="true" /> SFX
          </button>
        </div>
        {root === "music" ? (
          <div className="library-toolbar-search">
            <span className="library-toolbar-search-icon" aria-hidden="true">
              <SearchIcon />
            </span>
            <input
              type="search"
              value={pendingQuery}
              onChange={(e) => setPendingQuery(e.target.value)}
              placeholder="Search title, artist, composer, album, date or path"
              aria-label="Search music library"
            />
            {pendingQuery !== "" ? (
              <button
                type="button"
                className="library-toolbar-search-clear"
                onClick={clearSearch}
                aria-label="Clear search"
                title="Clear"
              >
                <XIcon />
              </button>
            ) : null}
          </div>
        ) : null}
        {root === "music" ? (
          <IconButton
            label="Create and review database mood tags"
            icon={<TagIcon />}
            className="library-mood-tagging-btn"
            onClick={() => setMoodTaggingOpen(true)}
          >
            Mood tags
          </IconButton>
        ) : null}
        {root === "music" ? (
          <IconButton
            label="Find and batch-fix common filename/tag issues"
            icon={<SparkleIcon />}
            className="library-cleanup-btn"
            onClick={() => {
              void Promise.resolve(navigate("/assistant/cleanup/run", {
                state: {
                  cleanupScope: {
                    path,
                    trackIds: [...checked],
                  },
                },
              })).catch((error: unknown) => {
                toast.error("Navigation failed", error instanceof Error ? error.message : undefined);
              });
            }}
          >
            Clean up
          </IconButton>
        ) : null}
        {root === "music" ? (
          <ConvertLibraryButton onFinished={() => setRefreshKey((k) => k + 1)} />
        ) : null}
        <RescanButton root={root} onComplete={() => setRefreshKey((k) => k + 1)} />
      </header>

      {moodTaggingOpen && root === "music" ? (
        <Suspense fallback={<RouteSpinner />}>
          <MoodTaggingDialog
            path={path}
            checkedIds={[...checked]}
            onClose={() => setMoodTaggingOpen(false)}
            onChanged={() => setRefreshKey((key) => key + 1)}
          />
        </Suspense>
      ) : null}

      {root === "music" ? (
        <MusicWorkspace
          path={path}
          onPathChange={setPath}
          query={query}
          refreshKey={refreshKey}
          onRefresh={() => setRefreshKey((k) => k + 1)}
          checked={checked}
          setChecked={setChecked}
          onRevealFolder={(p) => {
            // Search result → browse mode at that folder; the tree's
            // auto-reveal expands down to it.
            clearSearch();
            setPath(p);
          }}
        />
      ) : (
        <SfxBrowser
          path={path}
          onPathChange={setPath}
          refreshKey={refreshKey}
          onRefresh={() => setRefreshKey((k) => k + 1)}
        />
      )}
    </div>
  );
}

// --- shared bits ---------------------------------------------------------

function ConvertLibraryButton({ onFinished }: { onFinished: () => void }) {
  const [busy, setBusy] = useState(false);
  return (
    <IconButton
      label="Convert lossless and high-bitrate tracks to lighter streaming copies"
      icon={<CompressIcon />}
      className="library-convert-btn"
      disabled={busy}
      onClick={() => {
        void (async () => {
          const ok = await confirmDialog({
            title: "Convert the library for streaming?",
            body:
              "Lossless and high-bitrate tracks become 192 kbps AAC copies in the library. " +
              "Each original is kept in the originals folder and can be restored from the " +
              "track selection. Lighter files are left as they are.",
            confirmLabel: "Convert",
          });
          if (!ok) return;
          setBusy(true);
          try {
            const job = await libraryApi.convert();
            toast.info("Converting the library", "This runs in the background.");
            await followConversion(job.id, onFinished);
          } catch (e) {
            toast.error("Conversion did not start", e instanceof Error ? e.message : undefined);
          } finally {
            setBusy(false);
          }
        })();
      }}
    >
      {busy ? "Converting…" : "Convert"}
    </IconButton>
  );
}

function RescanButton({
  root,
  onComplete,
}: {
  root: Root;
  onComplete: () => void;
}) {
  const [busy, setBusy] = useState(false);
  if (root === "sfx") {
    // SFX doesn't have an index — refreshing the tree is enough.
    return (
      <IconButton
        label="Refresh the folder view"
        icon={<RescanIcon />}
        className="library-rescan"
        onClick={onComplete}
      >
        Refresh
      </IconButton>
    );
  }
  return (
    <IconButton
      label="Rescan the music library from disk"
      icon={<RescanIcon />}
      className="library-rescan"
      disabled={busy}
      onClick={() => {
        void (async () => {
          setBusy(true);
          try {
            const r = await libraryApi.rescan();
            const parts: string[] = [];
            if (r.added) parts.push(`+${r.added} added`);
            if (r.updated) parts.push(`${r.updated} updated`);
            if (r.removed) parts.push(`-${r.removed} removed`);
            toast.success("Rescan complete", parts.join(", ") || "No changes.");
            onComplete();
          } catch (e) {
            toast.error("Rescan failed", e instanceof Error ? e.message : undefined);
          } finally {
            setBusy(false);
          }
        })();
      }}
    >
      {busy ? "Rescanning…" : "Rescan"}
    </IconButton>
  );
}

// --- MUSIC ---------------------------------------------------------------
