import { useEffect, useState } from "react";

import { modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import type { ModeSummary } from "@/core/types";
import { wsClient } from "@/core/ws";

export function ModePicker() {
  const [modes, setModes] = useState<ModeSummary[]>([]);
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);

  useEffect(() => {
    modesApi.list().then(setModes).catch(() => setModes([]));
  }, []);

  function onChange(value: string) {
    const next = value === "" ? null : value;
    wsClient.send({ type: "set_active_mode", mode_id: next });
  }

  return (
    <label className="mode-picker">
      <span>Mode</span>
      <select value={activeModeId ?? ""} onChange={(e) => onChange(e.target.value)}>
        <option value="">— none —</option>
        {modes.map((m) => (
          <option key={m.id} value={m.id}>
            {m.name}
          </option>
        ))}
      </select>
    </label>
  );
}
