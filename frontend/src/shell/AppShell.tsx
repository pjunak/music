import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router-dom";

import { ConfirmDialogHost } from "@/components/ConfirmDialog";
import { Toaster } from "@/components/Toaster";
import { useAuthStore } from "@/core/auth";
import { AudioEngine } from "@/core/audioEngine";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { useKeyboardShortcuts } from "@/core/useKeyboardShortcuts";
import { useSfxHotkeys } from "@/core/useSfxHotkeys";
import { wsClient } from "@/core/ws";
import { ControlsView } from "@/views/ControlsView";
import { DiagnosticsView } from "@/views/DiagnosticsView";
import { LibraryView } from "@/views/LibraryView";
import { MetadataView } from "@/views/MetadataView";
import { ModesView } from "@/views/ModesView";
import { PlayerView } from "@/views/PlayerView";
import { PlaylistsView } from "@/views/PlaylistsView";
import { PresetsView } from "@/views/PresetsView";
import { SettingsView } from "@/views/SettingsView";

import { Header } from "./Header";
import { NowPlayingBar } from "./NowPlayingBar";

/** Wrap the authoring tabs so guests get bounced to /login instead of
 *  silently failing on every API call. */
function RequireAuth({ children }: { children: React.ReactNode }) {
  const status = useAuthStore((s) => s.status);
  if (status !== "authenticated") return <Navigate to="/login" replace />;
  return <>{children}</>;
}

export default function AppShell() {
  const applyMessage = usePlayerStore((s) => s.applyMessage);
  const setStatus = usePlayerStore((s) => s.setStatus);
  const authStatus = useAuthStore((s) => s.status);
  const isGuest = authStatus !== "authenticated";

  // The WS captures cookies at upgrade time, so a sign-in/out that changes
  // the session cookie has no effect on an already-open socket — the server
  // keeps treating it as whatever tier it was on connect. Re-running this
  // effect when `authStatus` flips closes the old socket and opens a new
  // one, forcing a fresh handshake with the now-current cookie.
  useEffect(() => {
    const unsubMsg = wsClient.subscribe(applyMessage);
    const unsubStatus = wsClient.onStatus(setStatus);
    // Surface server-side errors (e.g. "device not registered with audio_output",
    // rejected actions, "guest cannot mutate") via the toast layer.
    const unsubErr = wsClient.subscribe((msg) => {
      if (msg.type === "error") {
        toast.error("Server rejected action", msg.detail);
      }
    });
    wsClient.connect();
    return () => {
      unsubMsg();
      unsubStatus();
      unsubErr();
      wsClient.disconnect();
    };
  }, [applyMessage, setStatus, authStatus]);

  useKeyboardShortcuts();
  useSfxHotkeys();

  return (
    <div className={`shell${isGuest ? " shell-guest" : ""}`}>
      <Header />
      <main className="app-main">
        <Routes>
          <Route index element={<PlayerView />} />
          <Route path="diagnostics" element={<DiagnosticsView />} />
          <Route
            path="library"
            element={
              <RequireAuth>
                <LibraryView />
              </RequireAuth>
            }
          />
          <Route
            path="metadata"
            element={
              <RequireAuth>
                <MetadataView />
              </RequireAuth>
            }
          />
          <Route
            path="playlists"
            element={
              <RequireAuth>
                <PlaylistsView />
              </RequireAuth>
            }
          />
          <Route
            path="modes"
            element={
              <RequireAuth>
                <ModesView />
              </RequireAuth>
            }
          />
          <Route
            path="presets"
            element={
              <RequireAuth>
                <PresetsView />
              </RequireAuth>
            }
          />
          <Route
            path="controls"
            element={
              <RequireAuth>
                <ControlsView />
              </RequireAuth>
            }
          />
          <Route
            path="settings"
            element={
              <RequireAuth>
                <SettingsView />
              </RequireAuth>
            }
          />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
      </main>
      <NowPlayingBar />
      <AudioEngine />
      <Toaster />
      <ConfirmDialogHost />
    </div>
  );
}
