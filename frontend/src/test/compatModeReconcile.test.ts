/** Reconciliation tests for the real public/compat-mode.js: drive it over a
 *  fake WebSocket and assert the position_epoch contract on its <audio>
 *  elements. The bug class under guard: volume changes / queue edits used to
 *  restart the song (or re-seek every poll), because the old code inferred
 *  seeks by comparing positions. Now: same epoch → never touch the element;
 *  epoch changed → seek to the broadcast position. */
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

function readProjectFile(relPath: string): string {
  for (const root of [process.cwd(), resolve(process.cwd(), "frontend")]) {
    const p = resolve(root, relPath);
    if (existsSync(p)) return readFileSync(p, "utf8");
  }
  throw new Error(`could not locate ${relPath} from cwd ${process.cwd()}`);
}

const COMPAT_MODE_SOURCE = readProjectFile("public/compat-mode.js");

interface SentAction {
  type: string;
  [key: string]: unknown;
}

class FakeWebSocket {
  static instances: FakeWebSocket[] = [];
  url: string;
  readyState = 0;
  sent: SentAction[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;

  constructor(url: string) {
    this.url = url;
    FakeWebSocket.instances.push(this);
  }

  send(data: string): void {
    this.sent.push(JSON.parse(data) as SentAction);
  }

  close(): void {
    /* not needed */
  }

  // test helpers
  open(): void {
    this.readyState = 1;
    this.onopen?.();
  }

  push(msg: unknown): void {
    this.onmessage?.({ data: JSON.stringify(msg) });
  }
}

interface LaneOverrides {
  ambient?: Record<string, unknown>;
  [key: string]: unknown;
}

function makeState(epoch: number, overrides: LaneOverrides = {}, clientId = ""): unknown {
  const { ambient, ...rest } = overrides;
  return {
    revision: 1,
    position_epoch: epoch,
    is_playing: true,
    volume: 1,
    default_device_volume: 1,
    active_mode_id: null,
    active_output_device_ids: clientId ? [clientId] : [],
    device_volumes: {},
    active_soundboard_id: null,
    active_preset_ids: [],
    crossfade_ms: 0,
    crossfade_type: "linear",
    ambient: {
      current_track_id: 7,
      queue: [],
      history: [],
      position_ms: 0,
      loop: "off",
      shuffle: "off",
      source_playlist_id: null,
      ...(ambient ?? {}),
    },
    interrupt: null,
    looping_sfx: [],
    last_position_report: null,
    connected_devices: [],
    ...rest,
  };
}

/** Boot compat mode under ?compat, press start, open the fake socket, and
 *  return the socket + the engine's audio elements + this device's id. */
function bootCompatPlayer(): {
  ws: FakeWebSocket;
  clientId: string;
  audioEls: HTMLAudioElement[];
} {
  window.history.pushState(null, "", "/?compat");
  document.body.innerHTML = "";
  const root = document.createElement("div");
  root.id = "root";
  document.body.appendChild(root);

  new Function(COMPAT_MODE_SOURCE)();
  document.dispatchEvent(new Event("DOMContentLoaded"));
  const startButton = root.querySelector("button");
  expect(startButton).not.toBeNull();
  startButton?.click();

  const ws = FakeWebSocket.instances[0];
  expect(ws).toBeDefined();
  ws.open();
  const register = ws.sent.find((a) => a.type === "register");
  expect(register).toBeDefined();
  const clientId = register?.client_id as string;

  const audioEls = Array.from(document.querySelectorAll("audio"));
  expect(audioEls.length).toBe(2);
  return { ws, clientId, audioEls };
}

/** The element currently loaded with a library stream (the engine's active
 *  channel from the outside). */
function activeEl(audioEls: HTMLAudioElement[]): HTMLAudioElement {
  const el = audioEls.find((a) => a.src.includes("/api/library/tracks/"));
  expect(el).toBeDefined();
  return el as HTMLAudioElement;
}

beforeEach(() => {
  vi.useFakeTimers();
  delete window.__SPA_BOOTED__;
  delete window.__COMPAT_MODE_ACTIVE__;
  window.localStorage.clear();
  FakeWebSocket.instances = [];
  vi.stubGlobal("WebSocket", FakeWebSocket);
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  document.body.innerHTML = "";
});

describe("compat-mode.js epoch reconciliation", () => {
  it("loads the ambient track from a snapshot", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({ type: "state_snapshot", your_device_id: "", state: makeState(0, {}, clientId) });
    expect(activeEl(audioEls).src).toContain("/api/library/tracks/7/stream");
  });

  it("does NOT seek or reload on a same-epoch broadcast (volume change)", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({ type: "state_snapshot", your_device_id: "", state: makeState(3, {}, clientId) });
    const el = activeEl(audioEls);
    el.currentTime = 33; // simulate half a minute of local playback

    // Volume nudge: same epoch, and (server clock) a position way past the
    // element's — the old gate would have yanked playback. Must be inert.
    ws.push({
      type: "state_changed",
      state: makeState(
        3,
        { device_volumes: { [clientId]: 0.4 }, ambient: { position_ms: 95_000 } },
        clientId,
      ),
    });
    expect(el.currentTime).toBe(33);
    expect(el.src).toContain("/api/library/tracks/7/stream");
  });

  it("ignores a lower-revision state change", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({
      type: "state_snapshot",
      state: makeState(0, { revision: 2, device_volumes: { [clientId]: 0.4 } }, clientId),
    });
    const el = activeEl(audioEls);

    ws.push({
      type: "state_changed",
      state: makeState(0, { revision: 1, device_volumes: { [clientId]: 0.9 } }, clientId),
    });
    expect(el.volume).toBe(0.4);
  });

  it("uses legacy master times trim when the absolute-volume marker is absent", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({
      type: "state_snapshot",
      state: makeState(
        0,
        {
          volume: 0.2,
          default_device_volume: undefined,
          device_volumes: { [clientId]: 0.5 },
        },
        clientId,
      ),
    });
    expect(activeEl(audioEls).volume).toBe(0.1);
  });

  it("seeks when the epoch bumps, via the pending-seek path", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({ type: "state_snapshot", your_device_id: "", state: makeState(3, {}, clientId) });
    const el = activeEl(audioEls);

    ws.push({
      type: "state_changed",
      state: makeState(4, { ambient: { position_ms: 120_000 } }, clientId),
    });
    // jsdom elements report readyState 0 (no metadata), so the seek parks as
    // pending and lands on loadedmetadata — exactly the old-TV code path.
    el.dispatchEvent(new Event("loadedmetadata"));
    expect(el.currentTime).toBe(120);
  });

  it("plays the interrupt lane and returns ambient to its preserved position", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({ type: "state_snapshot", your_device_id: "", state: makeState(3, {}, clientId) });
    const el = activeEl(audioEls);

    // Interrupt fires (epoch bumps): the fallback swaps to the interrupt
    // track — no duck layering in compat mode, by design.
    ws.push({
      type: "state_changed",
      state: makeState(
        4,
        {
          interrupt: {
            current_track_id: 9,
            queue: [],
            position_ms: 0,
            return_to_ambient: true,
            fade_in_ms: 0,
            fade_out_ms: 0,
            duck_to: null,
          },
          ambient: { position_ms: 45_000 }, // frozen resume point
        },
        clientId,
      ),
    });
    expect(el.src).toContain("/api/library/tracks/9/stream");

    // Interrupt ends: back to ambient at the server-preserved position.
    ws.push({
      type: "state_changed",
      state: makeState(5, { ambient: { position_ms: 45_000 } }, clientId),
    });
    expect(el.src).toContain("/api/library/tracks/7/stream");
    el.dispatchEvent(new Event("loadedmetadata"));
    expect(el.currentTime).toBe(45);
  });

  it("reports its position once a second while playing as an active output", () => {
    const { ws, clientId, audioEls } = bootCompatPlayer();
    ws.push({ type: "state_snapshot", your_device_id: "", state: makeState(0, {}, clientId) });
    const el = activeEl(audioEls);
    el.currentTime = 12.3;

    vi.advanceTimersByTime(1100);
    const report = ws.sent.find((a) => a.type === "position_report");
    expect(report).toBeDefined();
    expect(report?.position_ms).toBe(12_300);
  });

  it("stays silent on reports when it is not an active output", () => {
    const { ws, audioEls } = bootCompatPlayer();
    // Active outputs don't include this device → it must not play or report.
    ws.push({ type: "state_snapshot", your_device_id: "", state: makeState(0, {}, "someone-else") });
    void audioEls;
    vi.advanceTimersByTime(2500);
    expect(ws.sent.filter((a) => a.type === "position_report")).toHaveLength(0);
  });
});
