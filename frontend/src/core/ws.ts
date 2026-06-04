import type { WsAction, WsMessage } from "@/core/types";
import { defaultDeviceName, useUiStore } from "@/core/uiStore";
import { validateWsMessage } from "@/core/wsValidate";

type Listener = (msg: WsMessage) => void;
type StatusListener = (status: WsStatus) => void;

export type WsStatus = "disconnected" | "connecting" | "connected";

const RECONNECT_DELAY_MS = 1500;

function buildWsUrl(path: string): string {
  const proto = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${proto}//${window.location.host}${path}`;
}

class WsClient {
  private ws: WebSocket | null = null;
  private listeners = new Set<Listener>();
  private statusListeners = new Set<StatusListener>();
  private reconnectTimeout: number | null = null;
  private intentionallyClosed = false;
  private status: WsStatus = "disconnected";

  connect(): void {
    this.intentionallyClosed = false;
    if (this.ws !== null && this.ws.readyState !== WebSocket.CLOSED) {
      return;
    }
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
    return false;
  }

  /** Re-send the `register` action with whatever the UI store currently
   *  reports for device name + capabilities. Idempotent on the server —
   *  replaces the prior registration for this connection. */
  sendRegister(): void {
    const { deviceName, capabilities } = useUiStore.getState();
    this.send({
      type: "register",
      name: deviceName ?? defaultDeviceName(),
      capabilities,
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

    ws.onopen = () => {
      this.setStatus("connected");
      this.sendRegister();
    };

    ws.onmessage = (event) => {
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
      this.ws = null;
      this.setStatus("disconnected");
      if (!this.intentionallyClosed) {
        this.reconnectTimeout = window.setTimeout(() => {
          this.openSocket();
        }, RECONNECT_DELAY_MS);
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
