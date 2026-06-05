import { useState } from "react";

import { FolderTree } from "./FolderTree";
import type { TreeFolder } from "./FolderTree";

/** Modal that picks a destination folder from the same FolderTree widget used
 *  in the Library. Reused for bulk-moving tracks and for re-parenting a whole
 *  folder, so the operator's mental model (same tree, same lazy loading, same
 *  root semantics) is identical everywhere. */
export function FolderPickerModal({
  title,
  body,
  confirmVerb = "Move",
  loadChildren,
  busy = false,
  initialDest = "",
  disableDest,
  onCancel,
  onConfirm,
}: {
  title: string;
  body?: string;
  /** Verb shown on the confirm button, e.g. "Move" → "Move to (root)". */
  confirmVerb?: string;
  loadChildren: (p: string) => Promise<TreeFolder[]>;
  busy?: boolean;
  initialDest?: string;
  /** Optional guard: return a reason string to block confirming for `dest`. */
  disableDest?: (dest: string) => string | null;
  onCancel: () => void;
  onConfirm: (dest: string) => void;
}) {
  const [dest, setDest] = useState(initialDest);
  const blocked = disableDest ? disableDest(dest) : null;

  return (
    <div
      className="modal-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onCancel();
      }}
    >
      <div
        className="modal"
        role="dialog"
        aria-label={title}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <header className="modal-header">
          <h2>{title}</h2>
        </header>
        <div className="modal-body">
          {body ? <p className="muted small">{body}</p> : null}
          <div className="folder-picker-tree">
            <FolderTree
              selectedPath={dest}
              onSelect={setDest}
              loadChildren={loadChildren}
            />
          </div>
          {blocked !== null ? <p className="error small">{blocked}</p> : null}
        </div>
        <div className="modal-actions">
          <button type="button" onClick={onCancel} disabled={busy}>
            Cancel
          </button>
          <button
            type="button"
            className="btn-primary"
            onClick={() => onConfirm(dest)}
            disabled={busy || blocked !== null}
          >
            {busy
              ? "Working…"
              : `${confirmVerb} to ${dest === "" ? "(root)" : dest}`}
          </button>
        </div>
      </div>
    </div>
  );
}
