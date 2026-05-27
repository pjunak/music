import { useCallback, useEffect, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import { SoundboardEditor } from "@/components/SoundboardEditor";
import { modesApi } from "@/core/api";
import { toast } from "@/core/toast";
import type { ModeSummary } from "@/core/types";

/** Top-level soundboard browser + editor.
 *
 *  Mirrors the Playlists tab's two-pane shell: list every soundboard
 *  across every mode on the left, edit the selected one on the right
 *  via the same `<SoundboardEditor>` the Modes tab uses.
 *
 *  Soundboards live under modes (`modes/<id>/soundboards/<sb>.yaml`), so
 *  *creating* one still happens from the Modes tab — that flow needs a
 *  mode picker, an id, and a name, which is awkward to bolt on here.
 *  This tab is for the common case: tweak existing soundboards without
 *  drilling through the Modes hierarchy each time. */

interface BoardEntry {
  modeId: string;
  modeName: string;
  boardId: string;
  boardName: string;
  itemCount: number;
}

export function SoundboardsView() {
  const [boards, setBoards] = useState<BoardEntry[]>([]);
  const [selected, setSelected] = useState<{ modeId: string; boardId: string } | null>(
    null,
  );
  const [loading, setLoading] = useState(false);
  const [refreshKey, setRefreshKey] = useState(0);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const modes: ModeSummary[] = await modesApi.list();
      // Pull each mode's full detail in parallel so we can flatten its
      // soundboards into a single list. Modes are typically a handful, so
      // the fan-out is negligible.
      const details = await Promise.all(
        modes.map((m) =>
          modesApi
            .get(m.id)
            .then((d) => ({ summary: m, detail: d }))
            .catch(() => null),
        ),
      );
      const flat: BoardEntry[] = [];
      for (const entry of details) {
        if (entry === null) continue;
        for (const sb of Object.values(entry.detail.soundboards)) {
          flat.push({
            modeId: entry.summary.id,
            modeName: entry.summary.name,
            boardId: sb.id,
            boardName: sb.name || sb.id,
            itemCount: sb.categories.reduce((acc, c) => acc + c.items.length, 0),
          });
        }
      }
      flat.sort((a, b) => {
        const m = a.modeName.localeCompare(b.modeName);
        return m !== 0 ? m : a.boardName.localeCompare(b.boardName);
      });
      setBoards(flat);
      // Drop the selection if it disappeared (mode deleted, board renamed).
      if (
        selected !== null &&
        !flat.some(
          (b) => b.modeId === selected.modeId && b.boardId === selected.boardId,
        )
      ) {
        setSelected(null);
      }
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    } finally {
      setLoading(false);
    }
  }, [selected]);

  useEffect(() => {
    void refresh();
    // The dep on `refreshKey` lets the SoundboardEditor force a reload after
    // it mutates a soundboard (so the item-count badge in the list updates).
  }, [refresh, refreshKey]);

  function bumpRefresh() {
    setRefreshKey((k) => k + 1);
  }

  return (
    <div className="two-pane-view soundboards-view">
      <div className="two-pane-pane soundboards-list-pane">
        <header className="playlists-header">
          <h2>Soundboards</h2>
          <span className="muted small">{boards.length}</span>
        </header>
        <p className="muted small">
          Edit any soundboard from any mode. Create new ones from the{" "}
          <strong>Modes</strong> tab.
        </p>
        <ul className="playlist-list">
          {loading && boards.length === 0 ? (
            <li className="muted small empty">Loading…</li>
          ) : boards.length === 0 ? (
            <li className="muted small empty">
              No soundboards yet — open the <strong>Modes</strong> tab and add
              one to a mode.
            </li>
          ) : (
            boards.map((b) => {
              const isSelected =
                selected?.modeId === b.modeId && selected?.boardId === b.boardId;
              return (
                <li
                  key={`${b.modeId}/${b.boardId}`}
                  className={`playlist-list-item ${isSelected ? "active" : ""}`}
                >
                  <button
                    type="button"
                    className="playlist-list-item-meta btn-ghost"
                    onClick={() =>
                      setSelected({ modeId: b.modeId, boardId: b.boardId })
                    }
                  >
                    <span className="playlist-name">{b.boardName}</span>
                    <span className="muted small">
                      mode <code>{b.modeId}</code> · {b.itemCount} item
                      {b.itemCount === 1 ? "" : "s"}
                    </span>
                  </button>
                </li>
              );
            })
          )}
        </ul>
      </div>

      <div className="two-pane-pane soundboards-detail-pane">
        {selected !== null ? (
          <SoundboardEditor
            key={`${selected.modeId}/${selected.boardId}`}
            modeId={selected.modeId}
            soundboardId={selected.boardId}
            breadcrumb={[
              {
                label: "All soundboards",
                onClick: () => {
                  setSelected(null);
                  bumpRefresh();
                },
              },
              {
                label:
                  boards.find(
                    (b) =>
                      b.modeId === selected.modeId && b.boardId === selected.boardId,
                  )?.modeName ?? selected.modeId,
              },
              { label: selected.boardId },
            ]}
          />
        ) : (
          <div className="empty-detail">
            <EmptyState title="No soundboard selected">
              Pick one from the list to edit its categories and items, or open
              the <strong>Modes</strong> tab to add a new soundboard to a mode.
            </EmptyState>
          </div>
        )}
      </div>
    </div>
  );
}
