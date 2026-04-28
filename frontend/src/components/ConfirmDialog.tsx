import { useEffect } from "react";
import { create } from "zustand";

interface ConfirmRequest {
  title: string;
  body?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** "danger" for destructive actions; styles the confirm button red. */
  tone?: "danger" | "primary";
  resolve: (ok: boolean) => void;
}

interface ConfirmStore {
  current: ConfirmRequest | null;
  open: (req: Omit<ConfirmRequest, "resolve">) => Promise<boolean>;
  resolve: (ok: boolean) => void;
}

const useConfirmStore = create<ConfirmStore>()((set, get) => ({
  current: null,
  open: (req) =>
    new Promise<boolean>((resolve) => {
      // If a confirm is already open, queue behaviour: reject the previous one.
      const prev = get().current;
      if (prev) prev.resolve(false);
      set({ current: { ...req, resolve } });
    }),
  resolve: (ok) => {
    const cur = get().current;
    if (cur) {
      cur.resolve(ok);
      set({ current: null });
    }
  },
}));

/** Promise-based confirm that pops a styled modal. Use this everywhere
 *  instead of `window.confirm` so the look matches the app and we can trap
 *  focus / keyboard correctly. */
export function confirmDialog(req: {
  title: string;
  body?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: "danger" | "primary";
}): Promise<boolean> {
  return useConfirmStore.getState().open(req);
}

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
