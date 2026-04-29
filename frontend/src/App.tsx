import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router-dom";

import { useAuthStore } from "@/core/auth";
import AppShell from "@/shell/AppShell";
import LoginPage from "@/shell/LoginPage";

/** Routing model:
 *
 *  - `/login`                   — public, redirects to `/` if already signed in.
 *  - `/diagnostics`             — public, no chrome (used as a TV-bookmark
 *                                  diagnostic page that also doesn't need auth).
 *  - everything else            — handled by `AppShell`. AppShell itself is
 *                                  rendered for both guests and signed-in
 *                                  users; it routes the Player view publicly
 *                                  and gates authoring tabs by sending guests
 *                                  to `/login`.
 *
 *  The auth refresh runs once at boot regardless of route — knowing whether
 *  there's a live session is what lets AppShell decide which tabs to expose.
 */
export default function App() {
  const { status, refresh } = useAuthStore();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (status === "unknown") {
    return <div className="centered">Loading…</div>;
  }

  return (
    <Routes>
      <Route
        path="/login"
        element={
          status === "authenticated" ? <Navigate to="/" replace /> : <LoginPage />
        }
      />
      <Route path="/*" element={<AppShell />} />
    </Routes>
  );
}
