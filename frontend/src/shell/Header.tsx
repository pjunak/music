import { useAuthStore } from "@/core/auth";
import { usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";
import { ModePicker } from "@/panels/ModePicker";
import { OutputPicker } from "@/panels/OutputPicker";

export function Header() {
  const user = useAuthStore((s) => s.user);
  const logout = useAuthStore((s) => s.logout);
  const wsStatus = usePlayerStore((s) => s.wsStatus);
  const volume = usePlayerStore((s) => s.state?.volume ?? 1.0);

  function onVolume(v: number) {
    wsClient.send({ type: "set_volume", volume: v });
  }

  return (
    <header className="app-header">
      <div className="app-header-left">
        <h1>Music</h1>
        <span className={`ws-status ws-status-${wsStatus}`}>{wsStatus}</span>
      </div>
      <div className="app-header-center">
        <ModePicker />
        <OutputPicker />
        <label className="volume-slider">
          <span>Vol</span>
          <input
            type="range"
            min="0"
            max="1"
            step="0.01"
            value={volume}
            onChange={(e) => onVolume(parseFloat(e.target.value))}
          />
        </label>
      </div>
      <div className="app-header-right">
        <span className="muted">{user?.username}</span>
        <button onClick={() => void logout()}>Sign out</button>
      </div>
    </header>
  );
}
