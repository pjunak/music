import { afterEach, describe, expect, it, vi } from "vitest";
import { wsClient } from "./ws";

class FakeSocket {
  static OPEN = 1;
  static CLOSED = 3;
  static instances: FakeSocket[] = [];
  readyState = 0;
  onmessage: ((event: MessageEvent<unknown>) => void) | null = null;
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor() { FakeSocket.instances.push(this); }
  send(): void { /* outbound traffic is irrelevant to frame validation */ }
  close(): void { this.readyState = FakeSocket.CLOSED; }
}

afterEach(() => {
  wsClient.disconnect();
  FakeSocket.instances = [];
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("WebSocket frame boundary", () => {
  it("rejects binary and malformed frames, then accepts valid text on the same connection", () => {
    vi.stubGlobal("WebSocket", FakeSocket);
    const warning = vi.spyOn(console, "warn").mockImplementation(() => {});
    const listener = vi.fn();
    const unsubscribe = wsClient.subscribe(listener);
    try {
      wsClient.connect();
      const socket = FakeSocket.instances[0];
      socket.onmessage?.(new MessageEvent("message", { data: new ArrayBuffer(4) }));
      socket.onmessage?.(new MessageEvent("message", { data: "not JSON" }));
      socket.onmessage?.(new MessageEvent("message", { data: '{"type":"unknown"}' }));
      expect(listener).not.toHaveBeenCalled();
      expect(warning).toHaveBeenCalledTimes(3);
      socket.onmessage?.(new MessageEvent("message", {
        data: JSON.stringify({ type: "error", detail: "session expired" }),
      }));
      expect(listener).toHaveBeenCalledExactlyOnceWith({ type: "error", detail: "session expired" });
    } finally {
      unsubscribe();
    }
  });
});
