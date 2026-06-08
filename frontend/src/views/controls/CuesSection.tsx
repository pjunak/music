import { useEffect, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import { modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type { Cue, ModeDetail } from "@/core/types";
import { useFireFlash } from "@/core/useFireFlash";
import { wsClient } from "@/core/ws";

/** Live Cues panel — one button per cue in the active mode. Clicking fires the
 *  cue (apply preset · start playlist from song/time · one-shot SFX · loops),
 *  resolved server-side. Authoring lives in Authoring → Cues. */
export function CuesSection() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [mode, setMode] = useState<ModeDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [firedId, flash] = useFireFlash();

  useEffect(() => {
    setError(null);
    if (activeModeId === null) {
      setMode(null);
      return;
    }
    let cancelled = false;
    void modesApi
      .get(activeModeId)
      .then((m) => {
        if (!cancelled) setMode(m);
      })
      .catch((e: unknown) => {
        if (!cancelled) {
          setMode(null);
          setError(e instanceof Error ? e.message : "failed to load mode");
        }
      });
    return () => {
      cancelled = true;
    };
  }, [activeModeId]);

  if (activeModeId === null) {
    return <EmptyState>Pick a mode in the strip above to use its cues.</EmptyState>;
  }
  if (error !== null) return <p className="error small">{error}</p>;
  if (mode === null) return <p className="muted small">Loading…</p>;

  const cues = Object.values(mode.cues);
  if (cues.length === 0) {
    return (
      <EmptyState>
        Mode <code>{activeModeId}</code> has no cues — add one in{" "}
        <strong>Authoring → Cues</strong>.
      </EmptyState>
    );
  }

  function fire(id: string, name: string) {
    wsClient.send({ type: "fire_cue", cue_id: id });
    flash(id);
    toast.info("Cue fired", name);
  }

  return (
    <div className="cues-section">
      <div className="cues-grid">
        {cues.map((c) => (
          <button
            key={c.id}
            type="button"
            className={`fire-tile${firedId === c.id ? " fired" : ""}`}
            onClick={() => fire(c.id, c.name)}
            title={c.description ?? undefined}
          >
            <span className="fire-tile-name">{c.name}</span>
            <span className="fire-tile-meta muted small">{cueSummary(c)}</span>
          </button>
        ))}
      </div>
    </div>
  );
}

function cueSummary(c: Cue): string {
  const parts: string[] = [];
  if (c.preset) parts.push(`preset ${c.preset}`);
  if (c.playlist) parts.push(`▶ ${c.playlist}`);
  if (c.loops?.length) {
    parts.push(`${c.loops.length} loop${c.loops.length === 1 ? "" : "s"}`);
  }
  if (c.sfx?.length) parts.push(`${c.sfx.length} sfx`);
  return parts.join(" · ") || "—";
}
