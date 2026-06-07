import { useConfirmStore } from "./confirmDialog";
import { Modal } from "./Modal";

/** Renders the open confirm dialog (if any). Mounts once at the AppShell
 *  level — the open API itself is `confirmDialog()` from `./confirmDialog`.
 *
 *  No Enter-to-confirm: a destructive dialog must be confirmed by an explicit
 *  click. Initial focus lands on Cancel for danger-tone, on the confirm
 *  button otherwise — so a stray Enter is always the safe choice. */
export function ConfirmDialogHost() {
  const current = useConfirmStore((s) => s.current);
  const resolve = useConfirmStore((s) => s.resolve);

  if (current === null) return null;

  const danger = (current.tone ?? "primary") === "danger";
  return (
    <Modal
      role="alertdialog"
      ariaLabel={current.title}
      title={current.title}
      onClose={() => resolve(false)}
      footer={
        <>
          <button
            type="button"
            onClick={() => resolve(false)}
            data-autofocus={danger ? true : undefined}
          >
            {current.cancelLabel ?? "Cancel"}
          </button>
          <button
            type="button"
            className={danger ? "btn-danger" : "btn-primary"}
            onClick={() => resolve(true)}
            data-autofocus={danger ? undefined : true}
          >
            {current.confirmLabel ?? "OK"}
          </button>
        </>
      }
    >
      {current.body !== undefined ? <p>{current.body}</p> : null}
    </Modal>
  );
}
