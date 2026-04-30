import type { ReactNode } from "react";

/** Empty / placeholder state for "no items yet" surfaces.
 *
 *  Used wherever a list, panel, or tab has nothing to show — playlists
 *  before the first one, presets before any are installed, scenes when
 *  the active mode declares none, etc. Centralising this keeps the
 *  visual language consistent (centred, padded, muted body) and gives
 *  one place to add icons, illustrations, or doc links later. */

interface Props {
  /** Short summary line (optional). */
  title?: string;
  /** Body text — typically a sentence telling the user how to get started. */
  children: ReactNode;
  /** Optional CTA / action node, rendered below the body. */
  action?: ReactNode;
}

export function EmptyState({ title, children, action }: Props) {
  return (
    <div className="empty-state">
      {title !== undefined ? <h3 className="empty-state-title">{title}</h3> : null}
      <div className="empty-state-body muted small">{children}</div>
      {action !== undefined ? <div className="empty-state-action">{action}</div> : null}
    </div>
  );
}
