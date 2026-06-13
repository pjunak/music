import { EmptyState } from "@/components/EmptyState";

/** Shown by the Authoring sub-tabs when no mode is active. Everything authored
 *  is per-mode, so without a mode there's nothing to show. */
export function NoModeEmpty({ kind }: { kind: string }) {
  return (
    <div className="empty-detail">
      <EmptyState title="No mode selected">
        {kind} live inside a mode. Pick or create a mode from the header (the
        mode dropdown + ⚙ Manage) to work on its {kind}.
      </EmptyState>
    </div>
  );
}
