import { usePlayerStore } from "@/core/playerStore";
import { wsClient } from "@/core/ws";

export function OutputPicker() {
  const devices = usePlayerStore((s) => s.state?.connected_devices ?? []);
  const activeIds = usePlayerStore((s) => s.state?.active_output_device_ids ?? []);
  const myDeviceId = usePlayerStore((s) => s.myDeviceId);

  const audioOutputs = devices.filter((d) => d.capabilities.includes("audio_output"));

  function toggle(deviceId: string) {
    const next = activeIds.includes(deviceId)
      ? activeIds.filter((d) => d !== deviceId)
      : [...activeIds, deviceId];
    wsClient.send({ type: "set_active_outputs", device_ids: next });
  }

  if (audioOutputs.length === 0) {
    return <span className="muted small">No audio outputs registered yet.</span>;
  }

  return (
    <div className="output-picker">
      <span className="output-picker-label">Outputs</span>
      <div className="output-picker-options">
        {audioOutputs.map((d) => {
          const checked = activeIds.includes(d.device_id);
          const isMe = d.device_id === myDeviceId;
          return (
            <label key={d.device_id} className="output-picker-option">
              <input
                type="checkbox"
                checked={checked}
                onChange={() => toggle(d.device_id)}
              />
              <span>
                {d.name}
                {isMe ? " (this)" : ""}
              </span>
            </label>
          );
        })}
      </div>
    </div>
  );
}
