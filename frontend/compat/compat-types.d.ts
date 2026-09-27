interface Window {
  __SPA_BOOTED__?: boolean;
  __COMPAT_MODE_ACTIVE__?: boolean;
}

interface CompatAudioElement extends HTMLAudioElement {
  __pendingSeekS?: number | null;
}

interface CompatTrackMetadata {
  display_title?: string | null;
  title?: string | null;
  artist?: string | null;
  album?: string | null;
  length_s?: number | null;
}

interface CompatLaneState {
  current_track_id?: number | null;
  position_ms?: number | null;
}

interface CompatPlayerState {
  revision?: number;
  position_epoch?: number;
  is_playing?: boolean;
  volume?: number;
  default_device_volume?: number;
  active_output_device_ids?: string[];
  device_volumes?: Record<string, number>;
  crossfade_ms?: number;
  ambient?: CompatLaneState;
  interrupt?: CompatLaneState | null;
}

interface CompatServerMessage {
  type?: string;
  state?: CompatPlayerState;
}

interface CompatClientAction {
  type: string;
  [key: string]: string | number;
}

type CompatSend = (action: CompatClientAction) => void;
type CompatStatus = (text: string, color: string) => void;

interface CompatWsHandlers {
  onStatus: CompatStatus;
  onOpen: (send: CompatSend) => void;
  onMessage: (message: CompatServerMessage) => void;
  onGiveUp?: () => void;
}

interface CompatPollingHandlers {
  onState: (state: CompatPlayerState) => void;
  onStatus: CompatStatus;
  onHardFailure?: () => void;
}
