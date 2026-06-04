#!/usr/bin/env python3
"""Headless audio output for the music player.

Turns any Linux box with a speaker into an output: it follows the server's
canonical PlayerState over the WebSocket and plays the current track through
mpv. No login, no server changes — it connects as a guest (see ../README.md
for the protocol).

What it does:
  - connects to ws(s)://<server>/api/ws, registers with a stable `client_id`
    (persisted), so the operator can designate this device as an audio output
    once and have it stick across restarts,
  - on every state change, plays the active track (interrupt lane wins over
    ambient) at the reported position, pausing/seeking to match the server,
  - plays fire-and-forget SFX (`sfx_fired`) layered over the music,
  - exposes a *local* on/off and volume (this speaker, not the server master),
  - optionally serves a tiny LAN HTTP control surface (on/off + volume +
    now-playing) so another device on the LAN — e.g. a dnd-table control panel
    — can drive it without any server credential.

It is deliberately a *dumb* player: ambient + SFX only, no crossfade/EQ/scene
effects (those live in the browser engine). That is the right trade-off for a
leave-it-on-a-shelf appliance.

Config is via environment variables (or the matching CLI flags):
  MUSIC_SERVER_URL   required, e.g. http://192.168.1.50:8000 or https://music.example
  MUSIC_OUTPUT_NAME  device name shown in the Console (default: hostname)
  MUSIC_CLIENT_ID    stable identity (default: generated + persisted to a dotfile)
  MUSIC_CONTROL_PORT if set, serve the LAN control endpoint on this port
  MUSIC_START_ON     "0" to boot muted (default boots playing)
  MUSIC_VOLUME       initial local volume 0..1 (default 1.0)

The operator must mark this device as an audio output in Settings → Devices
before it can play (output is fully manual — see ../README.md).

Dependencies: `websocket-client` and `python-mpv` (libmpv). See requirements.txt.
"""
from __future__ import annotations

import argparse
import json
import os
import socket
import threading
import urllib.parse
import urllib.request
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

try:
    import websocket  # websocket-client
except ImportError:  # pragma: no cover - import guard for a clearer message
    raise SystemExit(
        "missing dependency: pip install websocket-client (see requirements.txt)"
    )

try:
    import mpv  # python-mpv (libmpv)
except ImportError:  # pragma: no cover - import guard for a clearer message
    raise SystemExit(
        "missing dependency: pip install python-mpv, and apt install mpv libmpv2 "
        "(see requirements.txt / README.md)"
    )

# A position delta larger than this between what the server reports and where
# mpv actually is means a real seek happened (vs. normal playback drift), so we
# snap to it. Mirrors the browser engine's seek threshold.
SEEK_THRESHOLD_S = 1.5
RECONNECT_SECONDS = 3


def clamp01(v: float) -> float:
    return max(0.0, min(1.0, v))


# --------------------------------------------------------------------------- #
# Player backend                                                              #
# --------------------------------------------------------------------------- #
# Wrapped behind this thin interface so the WebSocket/reconcile logic never
# touches mpv directly — swapping in a GStreamer backend (already present on the
# dnd-table box) means reimplementing just these methods.


class MpvPlayer:
    """Audio-only mpv handle for the music lane."""

    def __init__(self) -> None:
        # idle=True keeps the instance alive with nothing loaded; vid='no' makes
        # it audio-only so it runs headless without a window/GPU.
        self._mpv = mpv.MPV(idle=True, vid="no", ytdl=False, audio_display="no")

    def play(self, url: str, start_s: float = 0.0) -> None:
        # `start=` does the initial seek atomically with the load, so we never
        # play a blip of the track's beginning before jumping to position.
        self._mpv.command("loadfile", url, "replace", f"start={max(0.0, start_s)}")
        self._mpv.pause = False

    def set_paused(self, paused: bool) -> None:
        self._mpv.pause = paused

    def stop(self) -> None:
        self._mpv.command("stop")

    def seek_abs(self, seconds: float) -> None:
        try:
            self._mpv.seek(max(0.0, seconds), reference="absolute")
        except Exception:
            # Seeking before the file is ready raises; harmless — the next
            # state push will re-evaluate.
            pass

    @property
    def time_pos(self) -> float | None:
        try:
            return self._mpv.time_pos
        except Exception:
            return None

    def set_volume(self, vol01: float) -> None:
        self._mpv.volume = clamp01(vol01) * 100.0


class SfxPlayer:
    """Separate mpv handle for fire-and-forget SFX, so a stinger doesn't
    interrupt the music lane. Overlapping SFX replace each other (a single
    handle) — fine for a dumb appliance."""

    def __init__(self) -> None:
        self._mpv = mpv.MPV(idle=True, vid="no", ytdl=False, audio_display="no")

    def fire(self, url: str, vol01: float) -> None:
        self._mpv.volume = clamp01(vol01) * 100.0
        self._mpv.command("loadfile", url, "replace")
        self._mpv.pause = False


# --------------------------------------------------------------------------- #
# Reconciler — the heart of the client                                        #
# --------------------------------------------------------------------------- #


class Reconciler:
    def __init__(
        self,
        server_url: str,
        player: MpvPlayer,
        *,
        client_id: str,
        respect_console: bool,
        local_on: bool,
        local_volume: float,
    ) -> None:
        self.server_url = server_url.rstrip("/")
        self.player = player
        self.respect_console = respect_console
        # Our stable identity — what the server keys active outputs on. The
        # snapshot's your_device_id is empty now, so we use this directly.
        self._client_id = client_id

        self._lock = threading.Lock()
        self._state: dict[str, Any] | None = None
        self._loaded_url: str | None = None
        self._meta_cache: dict[int, dict[str, Any]] = {}

        self.local_on = local_on
        self.local_volume = clamp01(local_volume)

    # -- inputs ------------------------------------------------------------- #

    def on_snapshot(self, state: dict[str, Any]) -> None:
        with self._lock:
            self._state = state
        self._reconcile()

    def on_state(self, state: dict[str, Any]) -> None:
        with self._lock:
            self._state = state
        self._reconcile()

    def set_local(self, *, on: bool | None = None, volume: float | None = None) -> None:
        with self._lock:
            if on is not None:
                self.local_on = on
            if volume is not None:
                self.local_volume = clamp01(volume)
        self._reconcile()

    # -- core --------------------------------------------------------------- #

    def stream_url(self, track_id: int) -> str:
        return f"{self.server_url}/api/library/tracks/{track_id}/stream"

    def _reconcile(self) -> None:
        with self._lock:
            state = self._state
            client_id = self._client_id
            local_on = self.local_on
            volume = self.local_volume
        if state is None:
            return

        interrupt = state.get("interrupt")
        ambient = state.get("ambient") or {}
        if interrupt:
            track_id = interrupt.get("current_track_id")
            position_ms = interrupt.get("position_ms", 0)
            playing = True  # an interrupt is, by definition, playing
        else:
            track_id = ambient.get("current_track_id")
            position_ms = ambient.get("position_ms", 0)
            playing = bool(state.get("is_playing"))

        # "Am I on?" — local switch by default; honour the server's active set
        # only when asked to (--respect-console).
        on = local_on
        if self.respect_console:
            on = on and client_id in (state.get("active_output_device_ids") or [])

        self.player.set_volume(volume)

        if not on or track_id is None or not playing:
            # Keep the loaded file so flipping back on resumes instantly; only a
            # genuinely empty lane clears it.
            if track_id is None:
                self.player.stop()
                with self._lock:
                    self._loaded_url = None
            else:
                self.player.set_paused(True)
            return

        url = self.stream_url(track_id)
        start_s = position_ms / 1000.0
        with self._lock:
            loaded = self._loaded_url
        if url != loaded:
            self.player.play(url, start_s=start_s)
            with self._lock:
                self._loaded_url = url
        else:
            cur = self.player.time_pos
            if cur is not None and abs(start_s - cur) > SEEK_THRESHOLD_S:
                self.player.seek_abs(start_s)
            self.player.set_paused(False)

    # -- control-surface helpers ------------------------------------------- #

    def control_status(self) -> dict[str, Any]:
        with self._lock:
            state = self._state
            on = self.local_on
            volume = self.local_volume
        track_id = None
        is_playing = False
        if state is not None:
            interrupt = state.get("interrupt")
            ambient = state.get("ambient") or {}
            track_id = (interrupt or ambient).get("current_track_id")
            is_playing = True if interrupt else bool(state.get("is_playing"))
        meta = self._meta(track_id) if track_id is not None else {}
        return {
            "on": on,
            "volume": volume,
            "is_playing": is_playing,
            "track_id": track_id,
            "title": meta.get("title"),
            "artist": meta.get("artist"),
        }

    def _meta(self, track_id: int) -> dict[str, Any]:
        """Best-effort track metadata for the now-playing line. Cached; failures
        are swallowed (the control surface still works without titles)."""
        if track_id in self._meta_cache:
            return self._meta_cache[track_id]
        meta: dict[str, Any] = {}
        try:
            with urllib.request.urlopen(
                f"{self.server_url}/api/library/tracks/{track_id}", timeout=2
            ) as resp:
                data = json.loads(resp.read())
            meta = {
                "title": data.get("display_title") or data.get("title"),
                "artist": data.get("artist"),
            }
            self._meta_cache[track_id] = meta
        except Exception:
            pass
        return meta


# --------------------------------------------------------------------------- #
# WebSocket loop                                                               #
# --------------------------------------------------------------------------- #


def ws_url_for(server_url: str) -> str:
    parsed = urllib.parse.urlparse(server_url)
    scheme = "wss" if parsed.scheme == "https" else "ws"
    return f"{scheme}://{parsed.netloc}/api/ws"


def run_ws(
    server_url: str,
    name: str,
    client_id: str,
    reconciler: Reconciler,
    sfx: SfxPlayer | None,
) -> None:
    url = ws_url_for(server_url)

    def on_open(ws: websocket.WebSocketApp) -> None:
        ws.send(json.dumps(
            {"type": "register", "name": name, "client_id": client_id}
        ))
        print(f"[ws] connected to {url}, registered as {name!r}", flush=True)

    def on_message(_ws: websocket.WebSocketApp, raw: str) -> None:
        try:
            msg = json.loads(raw)
        except (ValueError, TypeError):
            return
        kind = msg.get("type")
        if kind == "state_snapshot":
            reconciler.on_snapshot(msg.get("state") or {})
        elif kind == "state_changed":
            reconciler.on_state(msg.get("state") or {})
        elif kind == "sfx_fired" and sfx is not None:
            path = urllib.parse.quote(msg.get("item_path", ""))
            sfx.fire(
                f"{reconciler.server_url}/api/sfx/file?path={path}",
                clamp01(msg.get("volume", 1.0)) * reconciler.local_volume,
            )
        elif kind == "error":
            print(f"[ws] server error: {msg.get('detail')}", flush=True)

    def on_close(_ws: websocket.WebSocketApp, code: Any, reason: Any) -> None:
        print(f"[ws] closed ({code} {reason}); reconnecting…", flush=True)

    def on_error(_ws: websocket.WebSocketApp, err: Any) -> None:
        print(f"[ws] error: {err}", flush=True)

    app = websocket.WebSocketApp(
        url,
        on_open=on_open,
        on_message=on_message,
        on_close=on_close,
        on_error=on_error,
    )
    # reconnect= retries on drop with a fixed backoff, so the appliance recovers
    # from server restarts / network blips on its own.
    app.run_forever(reconnect=RECONNECT_SECONDS)


# --------------------------------------------------------------------------- #
# Optional LAN control surface                                                #
# --------------------------------------------------------------------------- #


class _ControlServer(ThreadingHTTPServer):
    reconciler: Reconciler


class _ControlHandler(BaseHTTPRequestHandler):
    server: _ControlServer  # type: ignore[assignment]

    def log_message(self, *_args: Any) -> None:  # quieter logs
        pass

    def _cors(self) -> None:
        # LAN-only single-user appliance: a wildcard origin lets the dnd-table
        # control panel (a different LAN origin) read/drive it without a proxy.
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")

    def _json(self, code: int, payload: dict[str, Any]) -> None:
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self._cors()
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_OPTIONS(self) -> None:  # noqa: N802 - http.server naming
        self.send_response(204)
        self._cors()
        self.end_headers()

    def do_GET(self) -> None:  # noqa: N802
        if self.path.split("?", 1)[0] != "/control":
            self._json(404, {"error": "not found"})
            return
        self._json(200, self.server.reconciler.control_status())

    def do_POST(self) -> None:  # noqa: N802
        if self.path.split("?", 1)[0] != "/control":
            self._json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length", 0) or 0)
        try:
            body = json.loads(self.rfile.read(length) or b"{}")
        except (ValueError, TypeError):
            self._json(400, {"error": "invalid JSON"})
            return
        on = body.get("on")
        volume = body.get("volume")
        self.server.reconciler.set_local(
            on=bool(on) if on is not None else None,
            volume=float(volume) if volume is not None else None,
        )
        self._json(200, self.server.reconciler.control_status())


def start_control_server(port: int, reconciler: Reconciler) -> None:
    server = _ControlServer(("0.0.0.0", port), _ControlHandler)
    server.reconciler = reconciler
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    print(f"[control] LAN control surface on http://0.0.0.0:{port}/control", flush=True)


# --------------------------------------------------------------------------- #
# Entry point                                                                 #
# --------------------------------------------------------------------------- #


def load_or_create_client_id(explicit: str | None) -> str:
    """Resolve this appliance's stable identity. Precedence: --client-id /
    MUSIC_CLIENT_ID, else a generated id persisted to a dotfile so the
    operator's output designation sticks across restarts. Falls back to an
    ephemeral id if the dotfile can't be written."""
    if explicit:
        return explicit
    state_dir = Path(
        os.environ.get("MUSIC_STATE_DIR")
        or (Path.home() / ".config" / "music-output")
    )
    path = state_dir / "client-id"
    try:
        if path.is_file():
            existing = path.read_text(encoding="utf-8").strip()
            if existing:
                return existing
    except OSError:
        pass
    new_id = f"headless-{uuid.uuid4()}"
    try:
        state_dir.mkdir(parents=True, exist_ok=True)
        path.write_text(new_id, encoding="utf-8")
    except OSError:
        pass  # ephemeral for this run — still works, just won't be remembered
    return new_id


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description="Headless audio output for the music player.")
    p.add_argument(
        "--server",
        default=os.environ.get("MUSIC_SERVER_URL"),
        help="player base URL, e.g. http://192.168.1.50:8000 (env MUSIC_SERVER_URL)",
    )
    p.add_argument(
        "--name",
        default=os.environ.get("MUSIC_OUTPUT_NAME") or socket.gethostname(),
        help="device name shown in the Console (env MUSIC_OUTPUT_NAME)",
    )
    p.add_argument(
        "--client-id",
        default=os.environ.get("MUSIC_CLIENT_ID"),
        help="stable identity (env MUSIC_CLIENT_ID; default: persisted dotfile)",
    )
    p.add_argument(
        "--control-port",
        type=int,
        default=int(os.environ["MUSIC_CONTROL_PORT"])
        if os.environ.get("MUSIC_CONTROL_PORT")
        else None,
        help="serve the LAN on/off+volume endpoint on this port (env MUSIC_CONTROL_PORT)",
    )
    p.add_argument(
        "--respect-console",
        action="store_true",
        help="only play when the operator has switched this device on in the Console "
        "(default: a local on/off, on by default)",
    )
    p.add_argument(
        "--no-sfx",
        action="store_true",
        help="don't play soundboard SFX events",
    )
    p.add_argument(
        "--start-off",
        action="store_true",
        default=os.environ.get("MUSIC_START_ON") == "0",
        help="boot muted (default boots playing) (env MUSIC_START_ON=0)",
    )
    p.add_argument(
        "--volume",
        type=float,
        default=float(os.environ.get("MUSIC_VOLUME", "1.0")),
        help="initial local volume 0..1 (env MUSIC_VOLUME)",
    )
    return p.parse_args()


def main() -> int:
    args = parse_args()
    if not args.server:
        raise SystemExit("error: --server (or MUSIC_SERVER_URL) is required")

    client_id = load_or_create_client_id(args.client_id)
    player = MpvPlayer()
    sfx = None if args.no_sfx else SfxPlayer()
    reconciler = Reconciler(
        args.server,
        player,
        client_id=client_id,
        respect_console=args.respect_console,
        local_on=not args.start_off,
        local_volume=args.volume,
    )

    if args.control_port is not None:
        start_control_server(args.control_port, reconciler)

    print(
        f"[output] {args.name!r} → {args.server} "
        f"(on={'console' if args.respect_console else not args.start_off}, "
        f"sfx={'off' if args.no_sfx else 'on'})",
        flush=True,
    )
    try:
        run_ws(args.server, args.name, client_id, reconciler, sfx)
    except KeyboardInterrupt:
        print("\n[output] bye", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
