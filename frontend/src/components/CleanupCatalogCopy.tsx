import { useMemo, useRef, useState } from "react";

import type { BackgroundJob } from "@/core/api";

/** A file-download-independent export of the original, unmerged catalog job. */
export function CleanupCatalogCopy({ job }: { job: BackgroundJob }) {
  const json = useMemo(() => JSON.stringify(job, null, 2), [job]);
  const [message, setMessage] = useState("");
  const textarea = useRef<HTMLTextAreaElement>(null);

  async function copy() {
    try {
      await navigator.clipboard.writeText(json);
      setMessage("Catalog results copied. Paste into a text file and save it as .json.");
    } catch {
      setMessage("Clipboard access is unavailable. Copy the selected JSON below and save it as a .json file.");
      textarea.current?.focus();
      textarea.current?.select();
    }
  }

  return (
    <details className="cleanup-catalog-copy">
      <summary>Copy or view catalog results</summary>
      <p className="muted small">
        If the download does not work in your browser, copy this complete result into a .json file.
        It includes this run's original proposals and evidence.
      </p>
      <button type="button" className="btn-ghost" onClick={() => void copy()}>Copy JSON</button>
      {message && <p role="status">{message}</p>}
      <textarea
        ref={textarea}
        aria-label="Catalog results JSON"
        value={json}
        readOnly
        rows={8}
        spellCheck={false}
        onFocus={(event) => event.currentTarget.select()}
      />
    </details>
  );
}
