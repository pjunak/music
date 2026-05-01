import { useEffect } from "react";

import { useConfirmStore } from "./confirmDialog";

/** Renders the open confirm dialog (if any). Mounts once at the AppShell
 *  level — the open API itself is `confirmDialog()` from `./confirmDialog`. */
export function ConfirmDialogHost() {
  const current = useConfirmStore((s) => s.current);
  const resolve = useConfirmStore((s) => s.resolve);

  useEffect(() => {
    if (current === null) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") resolve(false);
      if (e.key === "Enter") resolve(true);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [current, resolve]);

  if (current === null) return null;

  const tone = current.tone ?? "primary";
  return (
    <div className="modal-backdrop" onMouseDown={() => resolve(false)}>
      <div
        className="modal"
        role="alertdialog"
        aria-modal="true"
        aria-label={current.title}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-header">
          <h2>{current.title}</h2>
        </header>
        <div className="modal-body">
          {current.body !== undefined ? <p>{current.body}</p> : null}
        </div>
        <div className="modal-actions">
          <button type="button" onClick={() => resolve(false)} autoFocus>
            {current.cancelLabel ?? "Cancel"}
          </button>
          <button
            type="button"
            className={tone === "danger" ? "btn-danger" : "btn-primary"}
            onClick={() => resolve(true)}
          >
            {current.confirmLabel ?? "OK"}
          </button>
        </div>
      </div>
    </div>
  );
}
