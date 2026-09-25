import { toast } from "@/core/toast";

import { downloadJson } from "./downloadJson";

export function ListeningSampleExport({ trackIds }: { trackIds: readonly number[] }) {
  const valid = trackIds.length >= 2 && trackIds.length <= 1000
    && new Set(trackIds).size === trackIds.length
    && trackIds.every((id) => Number.isSafeInteger(id) && id > 0);

  function download() {
    if (!valid) return;
    try {
      downloadJson({
        schema_version: "song-mood-inventory/v1",
        track_ids: [...trackIds].sort((left, right) => left - right),
      }, "mood-listening-inventory.json");
    } catch {
      toast.error("Could not export the listening sample");
    }
  }

  return (
    <details className="assistant-bulk-starters">
      <summary>Listening comparison</summary>
      <p className="small muted">
        Select 2–1,000 tracks before comparing taggers. This export contains no tag suggestions.
        Export the saved vocabulary from the Vocabulary tab too.
      </p>
      <button type="button" className="btn-secondary" disabled={!valid} onClick={download}>
        Export listening sample
      </button>
    </details>
  );
}
