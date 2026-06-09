import { useEffect, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import { libraryApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { trackTitle } from "@/core/trackDisplay";
import type { Track } from "@/core/types";
import { wsClient } from "@/core/ws";

/** Interrupt control surface.
 *
 *  When an interrupt is active, shows what's playing and offers Skip /
 *  Cancel. When none is active, points the user at the Library tab — the
 *  ⚡ button on each track row fires it, and interrupt templates
 *  (Authoring → Interrupts) fire pre-configured ones. */
export function InterruptSection() {
  const interrupt = usePlayerStore((s) => s.state?.interrupt ?? null);
  const [track, setTrack] = useState<Track | null>(null);

  useEffect(() => {
    if (interrupt === null) {
      setTrack(null);
      return;
    }
    let cancelled = false;
    void libraryApi
      .getTrack(interrupt.current_track_id)
      .then((t) => {
        if (!cancelled) setTrack(t);
      })
      .catch(() => {
        if (!cancelled) setTrack(null);
      });
    return () => {
      cancelled = true;
    };
  }, [interrupt]);

  function cancel() {
    wsClient.send({ type: "cancel_interrupt" });
  }
  function skip() {
    wsClient.send({ type: "interrupt_skip_next" });
  }

  if (interrupt === null) {
    return (
      <EmptyState>
        No interrupt active. Fire one from the Library (the lightning button on a
        track row) or from a mode interrupt template.
      </EmptyState>
    );
  }

  return (
    <div className="interrupt-section" role="status" aria-live="polite">
      <div className="interrupt-active">
        <div className="interrupt-meta">
          <span className="track-title">
            {trackTitle(track) || `Track ${interrupt.current_track_id}`}
          </span>
          <span className="muted small">
            {track?.artist || ""}
            {interrupt.queue.length > 0
              ? ` · +${interrupt.queue.length} more in interrupt queue`
              : ""}
            {interrupt.return_to_ambient
              ? "  ·  resumes ambient on end"
              : "  ·  stops on end"}
          </span>
        </div>
        <div className="interrupt-actions">
          <button type="button" onClick={skip} title="Skip to next interrupt track">
            Skip
          </button>
          <button
            type="button"
            className="btn-danger"
            onClick={cancel}
            title="End interrupt now"
          >
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
