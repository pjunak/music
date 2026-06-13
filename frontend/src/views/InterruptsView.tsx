import { useCallback, useEffect, useState } from "react";

import { InterruptTemplatesEditor } from "@/components/InterruptTemplatesEditor";
import { NoModeEmpty } from "@/components/NoModeEmpty";
import { modesApi } from "@/core/api";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import type { ModeDetail } from "@/core/types";

/** Authoring → Interrupts. Edits the active mode's interrupt templates. */
export function InterruptsView() {
  const activeModeId = usePlayerStore((s) => s.state?.active_mode_id ?? null);
  const [detail, setDetail] = useState<ModeDetail | null>(null);

  const load = useCallback(async () => {
    if (activeModeId === null) {
      setDetail(null);
      return;
    }
    try {
      setDetail(await modesApi.get(activeModeId));
    } catch (e) {
      toast.error("Load failed", e instanceof Error ? e.message : undefined);
    }
  }, [activeModeId]);

  useEffect(() => {
    void load();
  }, [load]);

  if (activeModeId === null) return <NoModeEmpty kind="Interrupts" />;
  if (detail === null) return <p className="muted small">Loading…</p>;

  return (
    <div className="authoring-single">
      <InterruptTemplatesEditor
        modeId={activeModeId}
        detail={detail}
        onChanged={load}
      />
    </div>
  );
}
