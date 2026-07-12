import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router-dom";

import { ConfirmDialogHost } from "@/components/ConfirmDialogHost";
import { ConflictDialogHost } from "@/components/ConflictDialogHost";
import { ErrorBoundary } from "@/components/ErrorBoundary";
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
import { CuesView } from "@/views/CuesView";
import { DiagnosticsView } from "@/views/DiagnosticsView";
import { InterruptsView } from "@/views/InterruptsView";
import { LibraryView } from "@/views/LibraryView";
import { PlayerView } from "@/views/PlayerView";
import { PlaylistsView } from "@/views/PlaylistsView";
import { PresetsView } from "@/views/PresetsView";
import { SettingsView } from "@/views/SettingsView";
import { SoundboardsView } from "@/views/SoundboardsView";

import { Header } from "./Header";
import { indexTarget } from "./indexTarget";
import { LoginModal } from "./LoginModal";
import { NowPlayingBar } from "./NowPlayingBar";
import { LoginRedirect, Protected, RouteSpinner } from "./routeGuards";
import { SectionNav } from "./SectionNav";

// Old paths kept alive for bookmarks/external links — each redirects into the
// new IA. The Modes sub-tab is gone (mode CRUD moved to the header popup), so
// its old links land on the Authoring default.
const LEGACY_REDIRECTS: [string, string][] = [
  ["controls", "/console"],
  ["metadata", "/library"],
  ["library/files", "/library"],
  ["library/tags", "/library"],
  ["playlists", "/authoring/playlists"],
  ["soundboards", "/authoring/soundboards"],
  ["modes", "/authoring/playlists"],
  ["authoring/modes", "/authoring/playlists"],
  ["presets", "/authoring/presets"],
];

export default function AppShell() {
  const applyMessage = usePlayerStore((s) => s.applyMessage);
  const setStatus = usePlayerStore((s) => s.setStatus);
  const authStatus = useAuthStore((s) => s.status);
  const isAuthed = authStatus === "authenticated";
  // Guest styling only when we *know* the viewer is anonymous — not during the
  // brief "unknown" boot window (otherwise an authed reload flashes the guest
  // shell before snapping back).
  const isAnonymous = authStatus === "anonymous";

  // The WS captures cookies at upgrade time, so a sign-in/out that changes the
  // session cookie has no effect on an already-open socket — the server keeps
  // treating it as whatever tier it was on connect. Re-running this effect when
  // the *authenticated* boolean flips (login / logout) closes the old socket
  // and opens a new one, forcing a fresh handshake with the now-current cookie.
  // Keying on the boolean (not the 3-valued status) means a guest's
  // unknown→anonymous resolution doesn't trigger a pointless reconnect.
  useEffect(() => {
    const unsubMsg = wsClient.subscribe(applyMessage);
    const unsubStatus = wsClient.onStatus(setStatus);
    // Surface server-side errors (e.g. "device(s) not designated as outputs",
    // rejected actions, "guest cannot mutate") via the toast layer. Session
    // loss is the exception: the server tags those frames with a code, and
    // they trigger the re-login flow (store → anonymous, login modal opens)
    // instead of a dead-end error toast.
    const unsubErr = wsClient.subscribe((msg) => {
      if (msg.type !== "error") return;
      if (msg.code === "session_expired" || msg.code === "session_revoked") {
        useAuthStore
          .getState()
          .sessionLost(msg.code === "session_revoked" ? "revoked" : "expired");
        return;
      }
      toast.error("Server rejected action", msg.detail);
    });
    wsClient.connect();
    return () => {
      unsubMsg();
      unsubStatus();
      unsubErr();
      wsClient.disconnect();
    };
  }, [applyMessage, setStatus, isAuthed]);

  useKeyboardShortcuts();
  useSfxHotkeys();

  const shortcutSheetOpen = useUiTransient((s) => s.shortcutSheetOpen);
  const setShortcutSheetOpen = useUiTransient((s) => s.setShortcutSheetOpen);

  // What the bare `/` index renders, by auth status (decision in the
  // unit-tested `indexTarget`; this just maps it to the matching element).
  const idx = indexTarget(authStatus);
  const indexElement =
    idx === "console" ? (
      <Navigate to="/console" replace />
    ) : idx === "tv" ? (
      <PlayerView />
    ) : (
      <RouteSpinner />
    );

  return (
    <div className={`shell${isAnonymous ? " shell-guest" : ""}`}>
      <Header />
      <main className="app-main">
        {/* A crash in one view shows a recoverable error card in the main area
            while the Header / NowPlayingBar / AudioEngine stay mounted — so a
            buggy panel can't kill the whole session (or the music). */}
        <ErrorBoundary>
        <Routes>
          {/* `/` is the TV view for guests (the bookmark-on-a-room-display
              use case). For authed users it redirects to /console — the bare
              URL should land the operator on their workspace, not the
              read-only display surface. While auth is still resolving we show
              a spinner rather than flashing TV at someone who turns out to be
              the operator. */}
          <Route index element={indexElement} />
          {/* `/login` is no longer a page — sign-in is a modal. Keep the path
              working for old bookmarks by opening the modal and bouncing to /. */}
          <Route path="login" element={<LoginRedirect />} />
          {/* `/tv` is the canonical TV view, reachable by everyone — handy
              when an authed operator wants to preview what their room
              display looks like without losing their session. `/` is the
              guest-landing alias that redirects authed users to /console. */}
          <Route path="tv" element={<PlayerView />} />
          <Route path="diagnostics" element={<DiagnosticsView />} />
          <Route
            path="console"
            element={
              <Protected>
                <ControlsView />
              </Protected>
            }
          />
          {/* Library is one screen now — wide folder tree + Name/File list +
              a tag inspector that edits the selection. The old Files/Tags
              split is gone; redirect its sub-paths for bookmarks. */}
          <Route
            path="library"
            element={
              <Protected>
                <LibraryView />
              </Protected>
            }
          />
          {/* Authoring group — everything you set up *before* a session:
              playlists, mode bundles, soundboards, audio-effect presets. */}
          <Route
            path="authoring"
            element={
              <Protected>
                <SectionNav
                  ariaLabel="Authoring sections"
                  items={[
                    { to: "playlists", label: "Playlists" },
                    { to: "soundboards", label: "Soundboards" },
                    { to: "interrupts", label: "Interrupts" },
                    { to: "presets", label: "EQ Presets" },
                    { to: "cues", label: "Cues" },
                  ]}
                  action={
                    <button
                      type="button"
                      className="btn-ghost section-nav-import"
                      onClick={() =>
                        toast.info(
                          "Import",
                          "Importing from another mode isn't available yet.",
                        )
                      }
                      title="Import items from another mode (coming soon)"
                    >
                      ⬇ Import
                    </button>
                  }
                />
              </Protected>
            }
          >
            <Route index element={<Navigate to="playlists" replace />} />
            <Route path="playlists" element={<PlaylistsView />} />
            <Route path="soundboards" element={<SoundboardsView />} />
            <Route path="interrupts" element={<InterruptsView />} />
            <Route path="presets" element={<PresetsView />} />
            <Route path="cues" element={<CuesView />} />
          </Route>
          {/* Legacy routes — old top-level paths keep working for bookmarks
              and external links by redirecting into the new IA. */}
          {LEGACY_REDIRECTS.map(([from, to]) => (
            <Route key={from} path={from} element={<Navigate to={to} replace />} />
          ))}
          <Route
            path="settings"
            element={
              <Protected>
                <SettingsView />
              </Protected>
            }
          />
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
        </ErrorBoundary>
      </main>
      <NowPlayingBar />
      <AudioEngine />
      <Toaster />
      <LoginModal />
      <ConfirmDialogHost />
      <ConflictDialogHost />
      <InputDialogHost />
      {shortcutSheetOpen ? (
        <ShortcutSheet onClose={() => setShortcutSheetOpen(false)} />
      ) : null}
    </div>
  );
}
