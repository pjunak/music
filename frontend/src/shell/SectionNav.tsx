import { NavLink, Outlet } from "react-router-dom";

interface SubTab {
  /** Path relative to the parent route. */
  to: string;
  label: string;
  /** Optional `end` flag (passed to NavLink). Use for index routes. */
  end?: boolean;
}

interface Props {
  /** Aria label for the sub-tab nav strip. Should describe the section
   *  (e.g. "Library sections") so screen readers can announce context. */
  ariaLabel: string;
  items: SubTab[];
}

/** Sub-tab strip + outlet, used by the top-level Library and Authoring
 *  routes. The parent route renders this; its child routes get rendered
 *  into the outlet. Keeps the IA grouped without flattening every concern
 *  into the top-level tab strip. */
export function SectionNav({ ariaLabel, items }: Props) {
  return (
    <div className="section-nav-shell">
      <nav className="section-nav" aria-label={ariaLabel}>
        {items.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            end={t.end ?? false}
            className={({ isActive }) =>
              `section-nav-tab${isActive ? " section-nav-tab-active" : ""}`
            }
          >
            {t.label}
          </NavLink>
        ))}
      </nav>
      <div className="section-nav-body">
        <Outlet />
      </div>
    </div>
  );
}
