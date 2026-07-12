import { toast } from "@/core/toast";
import type { WsAction, WsMessage } from "@/core/types";
import { defaultDeviceName, useUiStore } from "@/core/uiStore";
import { validateWsMessage } from "@/core/wsValidate";

type Listener = (msg: WsMessage) => void;
type StatusListener = (status: WsStatus) => void;

export type WsStatus = "disconnected" | "connecting" | "connected";

// Reconnect backoff: start fast (a transient blip recovers near-instantly),
// then back off exponentially with jitter up to a ceiling so a server that's
// down for a while isn't hammered every 1.5s by every open tab.
const RECONNECT_BASE_MS = 1000;
const RECONNECT_MAX_MS = 30000;

// Actions whose loss during a disconnect is routine and self-healing —
// register re-fires on reconnect, position reports resume next tick. Anything
// else is an operator gesture (play, seek, fire cue…) that would otherwise
// evaporate silently while the socket is down.
const SILENT_DROP_TYPES = new Set<WsAction["type"]>(["register", "position_report"]);
const DROP_TOAST_THROTTLE_MS = 3000;

function buildWsUrl(path: string): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}${path}`;
}

class WsClient {
  private ws: WebSocket | null = null;
  private listeners = new Set<Listener>();
  private statusListeners = new Set<StatusListener>();
  private reconnectTimeout: number | null = null;
  private reconnectAttempts = 0;
  private intentionallyClosed = false;
  private status: WsStatus = "disconnected";
  private lastDropToastAt = 0;

  constructor() {
    // Reconnect the moment the tab comes back to life instead of waiting
    // out a backoff timer (which can sit at the 30s ceiling after a long
    // minimize). Covers: tab re-shown (visibilitychange/pageshow) and the
    // network coming back (online) — the "re-opened the app and the first
    // seconds are dead" case.
    if (typeof window !== "undefined") {
      window.addEventListener("online", this.wake);
      window.addEventListener("pageshow", this.wake);
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") this.wake();
      });
    }
  }

  private wake = (): void => {
    if (this.intentionallyClosed) return; // deliberately offline — stay put
    if (this.ws?.readyState === WebSocket.OPEN) return;
    // A handshake that was in flight when the network went away can wedge in
    // CONNECTING for tens of seconds. Abandon it (the identity guards in
    // openSocket ignore its late events) and dial fresh right now.
    const stale = this.ws;
    this.ws = null;
    if (stale !== null) {
      try {
        stale.close();
      } catch {
        /* already dead */
      }
    }
    this.connect();
  };

  connect(): void {
    this.intentionallyClosed = false;
    // A reconnect timer scheduled before an intentional connect would open a
    // second socket alongside this one — cancel it; we're connecting now.
    if (this.reconnectTimeout !== null) {
      window.clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
    if (this.ws !== null && this.ws.readyState !== WebSocket.CLOSED) {
      return;
    }
    this.reconnectAttempts = 0; // fresh intentional connect starts at base delay
    this.openSocket();
  }

  disconnect(): void {
    this.intentionallyClosed = true;
    if (this.reconnectTimeout !== null) {
      window.clearTimeout(this.reconnectTimeout);
      this.reconnectTimeout = null;
    }
    if (this.ws !== null) {
      this.ws.close();
      this.ws = null;
    }
    this.setStatus("disconnected");
  }

  /** Send an action. Returns whether it actually went out — `false` when the
   *  socket is absent or mid-(re)connect, so callers that need to know the
   *  server received it (e.g. position reports gating seek detection) can
   *  react instead of assuming success. */
  send(action: WsAction): boolean {
    if (this.ws?.readyState === WebSocket.OPEN) {
      this.ws.send(JSON.stringify(action));
      return true;
    }
    // Surface dropped operator gestures — a press during a reconnect window
    // otherwise vanishes with zero feedback. Throttled so a key-repeat burst
    // doesn't stack toasts.
    if (!SILENT_DROP_TYPES.has(action.type)) {
      const now = Date.now();
      if (now - this.lastDropToastAt > DROP_TOAST_THROTTLE_MS) {
        this.lastDropToastAt = now;
        toast.error("Not connected", "action not sent — reconnecting to the server");
      }
    }
    return false;
  }

  /** Re-send the `register` action with this browser's stable client_id and
   *  current device name. Idempotent on the server — re-binds the stable
   *  identity to this connection. */
  sendRegister(): void {
    const { deviceName, clientId } = useUiStore.getState();
    this.send({
      type: "register",
      name: deviceName ?? defaultDeviceName(),
      client_id: clientId,
    });
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  onStatus(listener: StatusListener): () => void {
    this.statusListeners.add(listener);
    listener(this.status); // immediate fire with current
    return () => {
      this.statusListeners.delete(listener);
    };
  }

  private openSocket(): void {
    this.setStatus("connecting");
    const ws = new WebSocket(buildWsUrl("/api/ws"));
    this.ws = ws;

    // Every handler below guards on `this.ws === ws`: a socket's close/open
    // events fire asynchronously, so a disconnect()+connect() pair (e.g. the
    // AppShell effect re-running on login state, or StrictMode's double
    // mount) lets the OLD socket's onclose land AFTER the new socket exists —
    // without the guard it would null the live reference and schedule a
    // spurious extra reconnect.
    ws.onopen = () => {
      if (this.ws !== ws) {
        ws.close(); // superseded while the handshake was in flight
        return;
      }
      this.reconnectAttempts = 0; // recovered — reset the backoff
      this.setStatus("connected");
      this.sendRegister();
    };

    ws.onmessage = (event) => {
      if (this.ws !== ws) return; // superseded socket flushing its last frames
      let raw: unknown;
      try {
        raw = JSON.parse(event.data);
      } catch (err) {
        // Malformed JSON. Log once so protocol-drift bugs are loud,
        // but don't tear down the connection.
        console.warn("[ws] failed to parse frame", err, event.data);
        return;
      }
      const msg = validateWsMessage(raw);
      if (msg === null) {
        // Server sent a frame the frontend doesn't recognise. This is
        // either a contract drift between backend and frontend, or
        // (more likely) a reverse proxy injecting an HTML error page
        // that happened to start with `{`. Either way, dropping is
        // safer than letting downstream listeners hit undefined.
        console.warn("[ws] rejected frame: shape doesn't match WsMessage", raw);
        return;
      }
      this.listeners.forEach((l) => {
        l(msg);
      });
    };

    ws.onclose = () => {
      if (this.ws !== ws) return; // a newer socket owns the state now
      this.ws = null;
      this.setStatus("disconnected");
      if (!this.intentionallyClosed) {
        const backoff = Math.min(
          RECONNECT_MAX_MS,
          RECONNECT_BASE_MS * 2 ** this.reconnectAttempts,
        );
        // Full jitter: a random delay in [0, backoff] so N tabs reconnecting
        // after a server restart don't all retry in lockstep.
        const delay = Math.round(Math.random() * backoff);
        this.reconnectAttempts += 1;
        this.reconnectTimeout = window.setTimeout(() => {
          this.openSocket();
        }, delay);
      }
    };

    ws.onerror = () => {
      // Let onclose handle reconnect logic uniformly.
      ws.close();
    };
  }

  private setStatus(status: WsStatus): void {
    if (this.status === status) return;
    this.status = status;
    this.statusListeners.forEach((l) => {
      l(status);
    });
  }
}

// Module-level singleton — a single WS connection per page load.
export const wsClient = new WsClient();
