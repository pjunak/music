import { useConflictStore } from "./conflictDialog";
import { Modal } from "./Modal";

/** Renders the open upload-conflict chooser (if any). Mounts once at the
 *  AppShell, like ConfirmDialogHost. The open API is `uploadConflictDialog()`
 *  from `./conflictDialog`.
 *
 *  Initial focus is Cancel (the safe choice) — Overwrite is destructive and
 *  must be a deliberate click, never a stray Enter. */
export function ConflictDialogHost() {
  const current = useConflictStore((s) => s.current);
  const resolve = useConflictStore((s) => s.resolve);

  if (current === null) return null;

  const { count, total, sampleNames } = current;
  const plural = count === 1 ? "" : "s";
  const more = count - sampleNames.length;

  return (
    <Modal
      role="alertdialog"
      ariaLabel="Files already exist"
      title="Files already exist"
      className="conflict-dialog"
      onClose={() => resolve(null)}
      footer={
        <>
          <button type="button" onClick={() => resolve(null)} data-autofocus>
            Cancel
          </button>
          <button
            type="button"
            className="btn-danger"
            onClick={() => resolve("overwrite")}
          >
            Overwrite
          </button>
          <button type="button" onClick={() => resolve("rename")}>
            Keep both
          </button>
          {/* Last child — the shared modal style accents it as the
              recommended action; skipping existing files is the safe,
              resume-a-failed-upload choice. */}
          <button type="button" onClick={() => resolve("skip")}>
            Skip existing
          </button>
        </>
      }
    >
      <p>
        {count === total
          ? `All ${count} file${plural} already exist in the destination.`
          : `${count} of ${total} files already exist in the destination.`}
      </p>
      {sampleNames.length > 0 ? (
        <ul className="conflict-names">
          {sampleNames.map((n) => (
            <li key={n}>{n}</li>
          ))}
          {more > 0 ? <li className="muted">+{more} more</li> : null}
        </ul>
      ) : null}
      <p className="muted small">
        <strong>Overwrite</strong> replaces them · <strong>Skip existing</strong>{" "}
        uploads only the new files · <strong>Keep both</strong> saves copies
        (adds “-1”).
      </p>
    </Modal>
  );
}
