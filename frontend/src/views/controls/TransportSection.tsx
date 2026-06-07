import { useEffect, useState } from "react";

import { usePlayerStore } from "@/core/playerStore";
import type { LoopMode } from "@/core/types";
import { wsClient } from "@/core/ws";

const LOOP_MODES: { value: LoopMode; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "queue", label: "Queue" },
  { value: "track", label: "Track" },
];

const CROSSFADE_TYPES: { value: string; label: string }[] = [
  { value: "linear", label: "Linear" },
  { value: "equal_power", label: "Equal power" },
  { value: "cut", label: "Cut" },
];

export function TransportSection() {
  const loop = usePlayerStore((s) => s.state?.ambient.loop ?? "off");
  const crossfadeMs = usePlayerStore((s) => s.state?.crossfade_ms ?? 0);
  const crossfadeType = usePlayerStore(
    (s) => s.state?.crossfade_type ?? "linear",
  );

  // Local mirror so the slider feels responsive while the user drags.
  const [localCrossfade, setLocalCrossfade] = useState(crossfadeMs);
  useEffect(() => setLocalCrossfade(crossfadeMs), [crossfadeMs]);

  function setLoop(mode: LoopMode) {
    wsClient.send({ type: "ambient_set_loop", loop: mode });
  }

  function setCrossfadeMs(ms: number) {
    setLocalCrossfade(ms);
    wsClient.send({ type: "set_crossfade", crossfade_ms: ms });
  }

  function setCrossfadeType(type: string) {
    wsClient.send({
      type: "set_crossfade",
      crossfade_ms: localCrossfade,
      crossfade_type: type,
    });
  }

  return (
    <div className="transport-section">
      <div className="transport-row">
        <span className="muted small">Loop</span>
        <div className="loop-toggles">
          {LOOP_MODES.map((m) => (
            <button
              key={m.value}
              type="button"
              className={`loop-toggle${loop === m.value ? " active" : ""}`}
              aria-pressed={loop === m.value}
              onClick={() => setLoop(m.value)}
            >
              {m.label}
            </button>
          ))}
        </div>
      </div>
      <div className="transport-row">
        <span className="muted small">Crossfade</span>
        <input
          type="range"
          min={0}
          max={10000}
          step={100}
          value={localCrossfade}
          onChange={(e) => setLocalCrossfade(parseInt(e.target.value, 10))}
          onMouseUp={(e) =>
            setCrossfadeMs(parseInt((e.target as HTMLInputElement).value, 10))
          }
          onTouchEnd={(e) =>
            setCrossfadeMs(parseInt((e.target as HTMLInputElement).value, 10))
          }
        />
        <span className="small num-readout">{(localCrossfade / 1000).toFixed(1)}s</span>
        <select
          value={crossfadeType}
          onChange={(e) => setCrossfadeType(e.target.value)}
        >
          {CROSSFADE_TYPES.map((t) => (
            <option key={t.value} value={t.value}>
              {t.label}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}
