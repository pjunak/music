import { useEffect } from "react";

import { AudioEngine } from "@/core/audioEngine";
import { usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";
import { LibraryPanel } from "@/panels/LibraryPanel";
import { PlaylistsPanel } from "@/panels/PlaylistsPanel";
import { QueuePanel } from "@/panels/QueuePanel";

import { Header } from "./Header";
import { NowPlayingBar } from "./NowPlayingBar";

export default function AppShell() {
  const applyMessage = usePlayerStore((s) => s.applyMessage);
  const setStatus = usePlayerStore((s) => s.setStatus);

  // Wire WS lifecycle to AppShell mount/unmount.
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

  return (
    <div className="shell">
      <Header />
      <main className="app-main">
        <aside className="app-sidebar">
          <PlaylistsPanel />
          <QueuePanel />
        </aside>
        <section className="app-content">
          <LibraryPanel />
        </section>
      </main>
      <NowPlayingBar />
      <AudioEngine />
    </div>
  );
}
