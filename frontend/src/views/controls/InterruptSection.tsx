import { useEffect, useState } from "react";

import { libraryApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { Track } from "@/core/types";
import { wsClient } from "@/core/ws";

/** Interrupt control surface.
 *
 *  When an interrupt is active, shows what's playing and offers a Cancel.
 *  When none is active, points the user at the Library tab — that's where
 *  you fire one (right-click a track → "Fire as interrupt", in a future UX).
 *  For now firing is via a track-id input as a fallback. */
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
      <p className="muted small">
        No interrupt active. Fire one from the Library tab.
      </p>
    );
  }

  return (
    <div className="interrupt-section">
      <div className="interrupt-active">
        <div className="interrupt-meta">
          <span className="track-title">
            {track?.title || track?.path || `Track ${interrupt.current_track_id}`}
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
          <button type="button" onClick={cancel} title="End interrupt now">
            Cancel
          </button>
        </div>
      </div>
    </div>
  );
}
