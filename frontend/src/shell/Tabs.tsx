import { NavLink } from "react-router-dom";

interface TabDef {
  to: string;
  label: string;
  end?: boolean;
}

const TABS: TabDef[] = [
  { to: "/", label: "Player", end: true },
  { to: "/library", label: "Library" },
  { to: "/playlists", label: "Playlists" },
  { to: "/modes", label: "Modes" },
  { to: "/presets", label: "Presets" },
  { to: "/controls", label: "Controls" },
  { to: "/settings", label: "Settings" },
];

export function Tabs() {
  return (
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
  );
}
