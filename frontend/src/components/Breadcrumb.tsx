/** Lightweight breadcrumb trail.
 *
 *  Items render left-to-right separated by a chevron. The last item is
 *  always non-interactive (the "you are here" leaf). Earlier items render
 *  as buttons when they carry an `onClick`, plain muted labels otherwise
 *  (e.g. a section name that exists in the path but isn't a navigable
 *  level on its own — "Modes › Tavern › Soundboards › Combat", where
 *  "Soundboards" is a label, not a clickable target).
 *
 *  Replaces the editor "← Back to mode" buttons; with the breadcrumb the
 *  operator can see *and* navigate to any ancestor in one component. */

export interface BreadcrumbItem {
  label: string;
  /** When present, the item renders as a button that calls this on click.
   *  When absent, it renders as a static muted label. */
  onClick?: () => void;
  /** Optional tooltip; defaults to the label. */
  title?: string;
}

interface Props {
  items: BreadcrumbItem[];
}

export function Breadcrumb({ items }: Props) {
  if (items.length === 0) return null;
  return (
    <nav className="breadcrumb" aria-label="Breadcrumb">
      <ol>
        {items.map((item, idx) => {
          const isLast = idx === items.length - 1;
          const isInteractive = !isLast && item.onClick !== undefined;
          return (
            <li key={idx} className="breadcrumb-item">
              {isInteractive ? (
                <button
                  type="button"
                  className="breadcrumb-link btn-ghost"
                  onClick={item.onClick}
                  title={item.title ?? item.label}
                >
                  {item.label}
                </button>
              ) : (
                <span
                  className={`breadcrumb-text${isLast ? " breadcrumb-current" : " muted"}`}
                  aria-current={isLast ? "page" : undefined}
                  title={item.title ?? item.label}
                >
                  {item.label}
                </span>
              )}
              {!isLast ? (
                <span className="breadcrumb-sep" aria-hidden="true">
                  ›
                </span>
              ) : null}
            </li>
          );
        })}
      </ol>
    </nav>
  );
}
