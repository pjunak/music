import { useEffect } from "react";

import { useAuthStore } from "@/core/auth";
import AppShell from "@/shell/AppShell";

/** Boot: kick the one-shot auth refresh, then hand off to AppShell.
 *
 *  AppShell renders for guests and signed-in users alike — the shell chrome is
 *  always on screen (no full-screen "Loading…" block, which on the dark theme
 *  read as a black screen for the length of the /api/auth/me round-trip). The
 *  public TV view renders immediately; protected routes gate their *content*
 *  inline (a spinner while auth resolves, a sign-in gate when anonymous) and
 *  sign-in is a modal. Nothing gates by navigation, so a cold load can't
 *  ping-pong between /console, /login and the TV view. */
export default function App() {
  const refresh = useAuthStore((s) => s.refresh);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return <AppShell />;
}
