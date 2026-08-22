import { useEffect, useMemo, useState } from "react";

import { toast } from "@/core/toast";

interface Props {
  label: string;
  report: object;
  openByDefault?: boolean;
}

export function TestResultReport({
  label,
  report,
  openByDefault = false,
}: Props) {
  const reportText = useMemo(() => JSON.stringify(report, null, 2), [report]);
  const [detailsOpen, setDetailsOpen] = useState(openByDefault);

  useEffect(() => {
    if (openByDefault) setDetailsOpen(true);
  }, [openByDefault, reportText]);

  async function copyReport() {
    try {
      await navigator.clipboard.writeText(reportText);
      toast.success("Test result copied", label);
    } catch {
      toast.error("Copy failed", "Clipboard access was blocked.");
    }
  }

  return (
    <section className="assistant-test-report" aria-label={label}>
      <div className="assistant-test-report-heading">
        <div>
          <strong>Test report</strong>
          <span>Safe troubleshooting JSON</span>
        </div>
        <button
          type="button"
          className="btn-ghost"
          aria-label={`Copy ${label}`}
          onClick={() => void copyReport()}
        >
          Copy result
        </button>
      </div>
      <details
        open={detailsOpen}
        onToggle={(event) => setDetailsOpen(event.currentTarget.open)}
      >
        <summary>Detailed result</summary>
        <pre aria-label={`${label} JSON`}>{reportText}</pre>
      </details>
    </section>
  );
}
