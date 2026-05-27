import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router-dom";

import { ConfirmDialogHost } from "@/components/ConfirmDialogHost";
import { InputDialogHost } from "@/components/InputDialogHost";
import { ShortcutSheet } from "@/components/ShortcutSheet";
import { Toaster } from "@/components/Toaster";
import { useAuthStore } from "@/core/auth";
import { AudioEngine } from "@/core/audioEngine";
import { usePlayerStore } from "@/core/playerStore";
import { toast } from "@/core/toast";
import { useKeyboardShortcuts } from "@/core/useKeyboardShortcuts";
import { useSfxHotkeys } from "@/core/useSfxHotkeys";
import { useUiTransient } from "@/core/uiTransient";
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
import { SoundboardsView } from "@/views/SoundboardsView";

import { Header } from "./Header";
import { NowPlayingBar } from "./NowPlayingBar";
import { SectionNav } from "./SectionNav";

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

  const shortcutSheetOpen = useUiTransient((s) => s.shortcutSheetOpen);
  const setShortcutSheetOpen = useUiTransient((s) => s.setShortcutSheetOpen);

  return (
    <div className={`shell${isGuest ? " shell-guest" : ""}`}>
      <Header />
      <main className="app-main">
        <Routes>
          {/* `/` is the TV view for guests (the bookmark-on-a-room-display
              use case). For authed users it redirects to /console — the bare
              URL should land the operator on their workspace, not the
              read-only display surface. */}
          <Route
            index
            element={
              isGuest ? <PlayerView /> : <Navigate to="/console" replace />
            }
          />
          {/* `/tv` is the canonical TV view, reachable by everyone — handy
              when an authed operator wants to preview what their room
              display looks like without losing their session. `/` is the
              guest-landing alias that redirects authed users to /console. */}
          <Route path="tv" element={<PlayerView />} />
          <Route path="diagnostics" element={<DiagnosticsView />} />
          <Route
            path="console"
            element={
              <RequireAuth>
                <ControlsView />
              </RequireAuth>
            }
          />
          {/* Library group — file management on the left, tag editing on
              the right. Sub-tab strip lives in SectionNav. */}
          <Route
            path="library"
            element={
              <RequireAuth>
                <SectionNav
                  ariaLabel="Library sections"
                  items={[
                    { to: "files", label: "Files" },
                    { to: "tags", label: "Tags" },
                  ]}
                />
              </RequireAuth>
            }
          >
            <Route index element={<Navigate to="files" replace />} />
            <Route path="files" element={<LibraryView />} />
            <Route path="tags" element={<MetadataView />} />
          </Route>
          {/* Authoring group — everything you set up *before* a session:
              playlists, mode bundles, soundboards, audio-effect presets. */}
          <Route
            path="authoring"
            element={
              <RequireAuth>
                <SectionNav
                  ariaLabel="Authoring sections"
                  items={[
                    { to: "playlists", label: "Playlists" },
                    { to: "soundboards", label: "Soundboards" },
                    { to: "modes", label: "Modes" },
                    { to: "presets", label: "Presets" },
                  ]}
                />
              </RequireAuth>
            }
          >
            <Route index element={<Navigate to="playlists" replace />} />
            <Route path="playlists" element={<PlaylistsView />} />
            <Route path="soundboards" element={<SoundboardsView />} />
            <Route path="modes" element={<ModesView />} />
            <Route path="presets" element={<PresetsView />} />
          </Route>
          {/* Legacy routes — old top-level paths keep working for bookmarks
              and external links by redirecting into the new IA. */}
          <Route path="controls" element={<Navigate to="/console" replace />} />
          <Route
            path="metadata"
            element={<Navigate to="/library/tags" replace />}
          />
          <Route
            path="playlists"
            element={<Navigate to="/authoring/playlists" replace />}
          />
          <Route
            path="soundboards"
            element={<Navigate to="/authoring/soundboards" replace />}
          />
          <Route
            path="modes"
            element={<Navigate to="/authoring/modes" replace />}
          />
          <Route
            path="presets"
            element={<Navigate to="/authoring/presets" replace />}
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
      <InputDialogHost />
      {shortcutSheetOpen ? (
        <ShortcutSheet onClose={() => setShortcutSheetOpen(false)} />
      ) : null}
    </div>
  );
}
