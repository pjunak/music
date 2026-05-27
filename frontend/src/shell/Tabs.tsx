import { useEffect, useRef, useState } from "react";
import { NavLink, useLocation } from "react-router-dom";

interface TabDef {
  to: string;
  label: string;
  end?: boolean;
}

/** Top-level authed tabs. Four groups:
 *
 *    Console   — the DM's live workspace (scenes, soundboard, transport).
 *    Library   — file management + tag editing (Files / Tags sub-tabs).
 *    Authoring — everything you set up before a session
 *                (Playlists / Soundboards / Modes / Presets sub-tabs).
 *    Settings  — preferences, sessions, backup, diagnostics link.
 *
 *  The TV (`/` index) is intentionally not in this list — it's the
 *  read-only display surface for guests / room displays, not somewhere a
 *  logged-in operator navigates to. Going there manually (via the bare URL
 *  or `/tv`) still works; AppShell redirects authed visitors from `/` back
 *  to /console.
 *
 *  Library and Authoring use `startsWith` matching so the top-level tab
 *  stays highlighted while you're on a sub-route. */
const TABS: TabDef[] = [
  { to: "/console", label: "Console" },
  { to: "/library", label: "Library" },
  { to: "/authoring", label: "Authoring" },
  { to: "/settings", label: "Settings" },
];

/** Tabs row.
 *
 *  On wide screens this paints a horizontal nav strip (the `.tabs` rule).
 *  On narrow widths the strip is hidden via CSS and `.tabs-mobile` shows
 *  a single hamburger button that pops a vertical menu — clearly readable
 *  vs the old "9 micro-tabs scrolling sideways" approach.
 *
 *  Both copies of the tab list render simultaneously; CSS picks which
 *  one the user actually sees. Keeps the React tree dumb (no resize
 *  observer needed) and means active-tab highlighting Just Works in
 *  whichever copy is visible. */
export function Tabs() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement | null>(null);
  const location = useLocation();

  // Close the menu on route change.
  useEffect(() => {
    setOpen(false);
  }, [location.pathname]);

  // Click-outside / Escape to close — only when open.
  useEffect(() => {
    if (!open) return;
    function onPointer(e: PointerEvent) {
      if (ref.current === null) return;
      if (e.target instanceof Node && ref.current.contains(e.target)) return;
      setOpen(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("pointerdown", onPointer);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("pointerdown", onPointer);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const activeLabel =
    TABS.find((t) =>
      t.end ? location.pathname === t.to : location.pathname.startsWith(t.to),
    )?.label ?? "Console";

  return (
    <>
      <nav className="tabs">
        {TABS.map((t) => (
          <NavLink
            key={t.to}
            to={t.to}
            end={t.end ?? false}
            className={({ isActive }) => `tab${isActive ? " tab-active" : ""}`}
          >
            {t.label}
          </NavLink>
        ))}
      </nav>

      <div className="tabs-mobile" ref={ref}>
        <button
          type="button"
          className="tabs-mobile-trigger"
          onClick={() => setOpen((v) => !v)}
          aria-haspopup="menu"
          aria-expanded={open}
          aria-label="Tabs menu"
        >
          <span className="tabs-mobile-burger" aria-hidden="true">
            <span />
            <span />
            <span />
          </span>
          <span className="tabs-mobile-label">{activeLabel}</span>
        </button>
        {open ? (
          <nav className="tabs-mobile-menu" role="menu">
            {TABS.map((t) => (
              <NavLink
                key={t.to}
                to={t.to}
                end={t.end ?? false}
                className={({ isActive }) =>
                  `tabs-mobile-item${isActive ? " tabs-mobile-item-active" : ""}`
                }
                role="menuitem"
              >
                {t.label}
              </NavLink>
            ))}
          </nav>
        ) : null}
      </div>
    </>
  );
}
