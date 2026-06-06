import { useEffect, useMemo, useState } from "react";

import { EmptyState } from "@/components/EmptyState";
import { IconButton } from "@/components/IconButton";
import { XIcon } from "@/components/icons";
import { modesApi } from "@/core/api";
import { usePlayerArray, usePlayerStore } from "@/core/playerStore";
import type { ModeDetail } from "@/core/types";
import { wsClient } from "@/core/ws";

/** The live LOOPS panel — every repeating SFX the server is firing on a timer,
 *  each with a stop button. The ➕ starts one ad-hoc (cues drop theirs in here
 *  too). Server-owned, so it survives a Console refresh. */
export function LoopsSection() {
  const loops = usePlayerArray((s) => s.state?.looping_sfx);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [adding, setAdding] = useState(false);
  const [mode, setMode] = useState<ModeDetail | null>(null);

  useEffect(() => {
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
      .catch(() => {
        if (!cancelled) setMode(null);
      });
    return () => {
      cancelled = true;
    };
  }, [activeModeId]);

  function stop(id: string) {
    wsClient.send({ type: "stop_loop", id });
  }

  const canAdd = mode !== null && Object.keys(mode.soundboards).length > 0;

  return (
    <div className="loops-panel">
      <div className="loops-bar">
        <span className="loops-title">LOOPS</span>
        <button
          type="button"
          className="loops-add"
          onClick={() => setAdding((a) => !a)}
          disabled={!canAdd}
          title={canAdd ? "Add a looping SFX" : "Pick a mode with a soundboard first"}
          aria-label="Add a looping SFX"
        >
          +
        </button>
      </div>

      {adding && mode !== null ? (
        <LoopAddForm mode={mode} onClose={() => setAdding(false)} />
      ) : null}

      {loops.length === 0 ? (
        <p className="loops-empty muted small">Nothing looping.</p>
      ) : (
        <ul className="loop-list">
          {loops.map((l) => (
            <li key={l.id} className="loop-row">
              <span className="loop-name" title={l.name}>
                {l.name}
              </span>
              <span className="loop-interval">{Math.round(l.interval_s)}s</span>
              <IconButton
                label={`Stop ${l.name}`}
                icon={<XIcon />}
                variant="danger"
                onClick={() => stop(l.id)}
              />
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function LoopAddForm({ mode, onClose }: { mode: ModeDetail; onClose: () => void }) {
  const soundboards = useMemo(() => Object.values(mode.soundboards), [mode]);
  const [sbId, setSbId] = useState(soundboards[0]?.id ?? "");
  const items = useMemo(() => {
    const sb = mode.soundboards[sbId];
    return sb ? sb.categories.flatMap((c) => c.items) : [];
  }, [mode, sbId]);
  const [itemPath, setItemPath] = useState(items[0]?.file ?? "");
  const [intervalS, setIntervalS] = useState(45);
  const [volume, setVolume] = useState(1);

  function pickSoundboard(id: string) {
    setSbId(id);
    const sb = mode.soundboards[id];
    const first = sb ? sb.categories.flatMap((c) => c.items)[0] : undefined;
    setItemPath(first?.file ?? "");
  }

  function add() {
    const item = items.find((i) => i.file === itemPath);
    if (!item) return;
    wsClient.send({
      type: "start_loop",
      id: crypto.randomUUID(),
      name: item.name,
      soundboard_id: sbId,
      item_path: itemPath,
      interval_s: Math.max(1, Math.round(intervalS)),
      volume,
    });
    onClose();
  }

  if (soundboards.length === 0) {
    return <EmptyState>This mode has no soundboards.</EmptyState>;
  }

  return (
    <div className="loop-add">
      <label>
        <span className="muted small">Soundboard</span>
        <select value={sbId} onChange={(e) => pickSoundboard(e.target.value)}>
          {soundboards.map((sb) => (
            <option key={sb.id} value={sb.id}>
              {sb.name || sb.id}
            </option>
          ))}
        </select>
      </label>
      <label>
        <span className="muted small">Sound</span>
        <select value={itemPath} onChange={(e) => setItemPath(e.target.value)}>
          {items.length === 0 ? <option value="">(no items)</option> : null}
          {items.map((it) => (
            <option key={it.file} value={it.file}>
              {it.name}
            </option>
          ))}
        </select>
      </label>
      <div className="loop-add-row">
        <label>
          <span className="muted small">Every (s)</span>
          <input
            type="number"
            min={1}
            max={3600}
            value={intervalS}
            onChange={(e) => setIntervalS(Number(e.target.value))}
          />
        </label>
        <label className="loop-add-vol">
          <span className="muted small">Vol {Math.round(volume * 100)}%</span>
          <input
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={volume}
            onChange={(e) => setVolume(Number(e.target.value))}
          />
        </label>
      </div>
      <div className="loop-add-actions">
        <button type="button" onClick={onClose}>
          Cancel
        </button>
        <button
          type="button"
          className="btn-primary"
          onClick={add}
          disabled={itemPath === ""}
        >
          Add loop
        </button>
      </div>
    </div>
  );
}
