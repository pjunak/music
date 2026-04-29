import { useEffect } from "react";
import { Route, Routes } from "react-router-dom";

import { ConfirmDialogHost } from "@/components/ConfirmDialog";
import { Toaster } from "@/components/Toaster";
import { AudioEngine } from "@/core/audioEngine";
import { usePlayerStore } from "@/core/playerStore";
import { useKeyboardShortcuts } from "@/core/useKeyboardShortcuts";
import { useSfxHotkeys } from "@/core/useSfxHotkeys";
import { wsClient } from "@/core/ws";
import { ControlsView } from "@/views/ControlsView";
import { LibraryView } from "@/views/LibraryView";
import { ModesView } from "@/views/ModesView";
import { PlayerView } from "@/views/PlayerView";
import { PlaylistsView } from "@/views/PlaylistsView";
import { PresetsView } from "@/views/PresetsView";
import { SettingsView } from "@/views/SettingsView";

import { Header } from "./Header";
import { NowPlayingBar } from "./NowPlayingBar";

export default function AppShell() {
  const applyMessage = usePlayerStore((s) => s.applyMessage);
  const setStatus = usePlayerStore((s) => s.setStatus);

  useEffect(() => {
    const unsubMsg = wsClient.subscribe(applyMessage);
    const unsubStatus = wsClient.onStatus(setStatus);
    wsClient.connect();
    return () => {
      unsubMsg();
      unsubStatus();
      wsClient.disconnect();
    };
  }, [applyMessage, setStatus]);

  useKeyboardShortcuts();
  useSfxHotkeys();

  return (
    <div className="shell">
      <Header />
      <main className="app-main">
        <Routes>
          <Route index element={<PlayerView />} />
          <Route path="library" element={<LibraryView />} />
          <Route path="playlists" element={<PlaylistsView />} />
          <Route path="modes" element={<ModesView />} />
          <Route path="presets" element={<PresetsView />} />
          <Route path="controls" element={<ControlsView />} />
          <Route path="settings" element={<SettingsView />} />
        </Routes>
      </main>
      <NowPlayingBar />
      <AudioEngine />
      <Toaster />
      <ConfirmDialogHost />
    </div>
  );
}
