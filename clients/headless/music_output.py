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
    ambient) at the server-clock position, seeking only when the server's
    position_epoch says a deliberate move happened,
  - plays fire-and-forget SFX (`sfx_fired`) layered over the music,
  - exposes a *local* on/off and volume (this speaker, not the server master),
  - optionally serves a tiny HTTP control surface (on/off + volume +
    now-playing) so a dnd-table control panel can drive it. Bound to loopback
    by default; expose it on the LAN with --control-bind 0.0.0.0 and protect
    it with --control-token.

It is deliberately a *dumb* player: ambient + SFX only, no crossfade/EQ/effect
colouring (those live in the browser engine). That is the right trade-off for a
leave-it-on-a-shelf appliance.

Config is via environment variables (or the matching CLI flags):
  MUSIC_SERVER_URL   required, e.g. http://192.168.1.50:8000 or https://music.example
  MUSIC_OUTPUT_NAME  device name shown in the Console (default: hostname)
  MUSIC_CLIENT_ID    stable identity (default: generated + persisted to a dotfile)
  MUSIC_CONTROL_PORT if set, serve the control endpoint on this port
  MUSIC_CONTROL_BIND control bind address (default 127.0.0.1; 0.0.0.0 for LAN)
  MUSIC_CONTROL_TOKEN require this token (X-Control-Token) on control requests
  MUSIC_START_ON     "0" to boot muted (default boots playing)
  MUSIC_VOLUME       initial local volume 0..1 (default 1.0)

The operator must mark this device as an audio output in Settings → Devices
before it can play (output is fully manual — see ../README.md).

Dependencies: `websocket-client` and `python-mpv` (libmpv). See requirements.txt.
"""
from __future__ import annotations

import argparse
import hmac
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

RECONNECT_SECONDS = 3
# Client-side liveness: websocket-client sends NO pings unless ping_interval
# is set, so a half-open connection (server power-loss, Wi-Fi drop with no
# TCP FIN) would block in recv indefinitely and the appliance would silently
# stop following state. With these, a dead peer trips ping_timeout and the
# run_forever reconnect takes over.
PING_INTERVAL_SECONDS = 20
PING_TIMEOUT_SECONDS = 10


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
        # position_epoch of the last state we reconciled while playing. The
        # server bumps it only on deliberate moves (seek/skip/interrupt), so
        # "seek iff it changed" replaces the old local-drift compare — which
        # used to yank playback back to a stale broadcast position on every
        # unrelated state change (volume, device joins, ...).
        self._last_epoch: int | None = None
        self._meta_cache: dict[int, dict[str, Any]] = {}
        self._has_state_this_connection = False

        self.local_on = local_on
        # Hardware-local gain remains available for appliance setup, but the
        # canonical software level comes from PlayerState.device_volumes.
        self.local_volume = clamp01(local_volume)

    # -- inputs ------------------------------------------------------------- #

    def begin_connection(self) -> None:
        with self._lock:
            self._has_state_this_connection = False

    def on_snapshot(self, state: dict[str, Any]) -> None:
        self.on_state(state)

    def on_state(self, state: dict[str, Any]) -> None:
        with self._lock:
            current_revision = (self._state or {}).get("revision", 0)
            if (
                self._has_state_this_connection
                and state.get("revision", 0) < current_revision
            ):
                return
            self._state = state
            self._has_state_this_connection = True
        self._reconcile()

    def set_local(self, *, on: bool | None = None, volume: float | None = None) -> None:
        with self._lock:
            if on is not None:
                self.local_on = on
            if volume is not None:
                self.local_volume = clamp01(volume)
        self._reconcile()

    def output_volume(self, event_volume: float = 1.0) -> float:
        """Canonical server volume folded with optional hardware-local gain."""
        with self._lock:
            state = self._state or {}
            local_volume = self.local_volume
            client_id = self._client_id
        volumes = state.get("device_volumes") or {}
        if "default_device_volume" in state:
            server_volume = volumes.get(client_id, state["default_device_volume"])
        else:
            # Old servers expose a global master and per-device trims.
            server_volume = state.get("volume", 1.0) * volumes.get(client_id, 1.0)
        return clamp01(event_volume) * clamp01(server_volume) * local_volume

    # -- core --------------------------------------------------------------- #

    def stream_url(self, track_id: int) -> str:
        return f"{self.server_url}/api/library/tracks/{track_id}/stream"

    def _reconcile(self) -> None:
        with self._lock:
            state = self._state
            client_id = self._client_id
            local_on = self.local_on
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

        self.player.set_volume(self.output_volume())

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
        epoch = state.get("position_epoch", 0)
        with self._lock:
            loaded = self._loaded_url
            last_epoch = self._last_epoch
        if url != loaded:
            # Track change or first load: position_ms is the server clock —
            # 0 on a natural advance, the real position on a mid-track join.
            self.player.play(url, start_s=start_s)
            with self._lock:
                self._loaded_url = url
        else:
            # Same track: seek iff the server says a deliberate move happened
            # (epoch changed). NEVER seek because positions look different —
            # the mpv clock is authoritative between epochs, and reports (or
            # simply the server's own dead-reckoning) keep the server in sync.
            if last_epoch is not None and epoch != last_epoch:
                self.player.seek_abs(start_s)
            self.player.set_paused(False)
        with self._lock:
            self._last_epoch = epoch

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
        reconciler.begin_connection()
        ws.send(json.dumps(
            {
                "type": "register",
                "name": name,
                "client_id": client_id,
                "protocol_version": 2,
            }
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
                reconciler.output_volume(msg.get("volume", 1.0)),
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
    app.run_forever(
        reconnect=RECONNECT_SECONDS,
        ping_interval=PING_INTERVAL_SECONDS,
        ping_timeout=PING_TIMEOUT_SECONDS,
    )


# --------------------------------------------------------------------------- #
# Optional LAN control surface                                                #
# --------------------------------------------------------------------------- #


_LOOPBACK_HOSTS = frozenset({"127.0.0.1", "localhost", "::1"})


class _ControlServer(ThreadingHTTPServer):
    reconciler: Reconciler
    token: str | None = None
    cors: bool = False  # only emit permissive CORS when bound off-loopback


class _ControlHandler(BaseHTTPRequestHandler):
    server: _ControlServer  # type: ignore[assignment]

    def log_message(self, *_args: Any) -> None:  # quieter logs
        pass

    def _cors(self) -> None:
        # Off by default: on loopback the control panel is same-origin, so a
        # wildcard would only let arbitrary web pages read this surface. When
        # the operator explicitly binds off-loopback we emit it (and allow the
        # token header) so a dnd-table panel on another LAN origin can drive it.
        if not self.server.cors:
            return
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type, X-Control-Token")

    def _authorized(self) -> bool:
        expected = self.server.token
        if not expected:
            return True
        got = self.headers.get("X-Control-Token", "")
        return hmac.compare_digest(got, expected)

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
        if not self._authorized():
            self._json(401, {"error": "unauthorized"})
            return
        self._json(200, self.server.reconciler.control_status())

    def do_POST(self) -> None:  # noqa: N802
        if self.path.split("?", 1)[0] != "/control":
            self._json(404, {"error": "not found"})
            return
        if not self._authorized():
            self._json(401, {"error": "unauthorized"})
            return
        length = int(self.headers.get("Content-Length", 0) or 0)
        try:
            body = json.loads(self.rfile.read(length) or b"{}")
        except (ValueError, TypeError):
            self._json(400, {"error": "invalid JSON"})
            return
        on = body.get("on")
        volume = body.get("volume")
        try:
            parsed_volume = float(volume) if volume is not None else None
        except (TypeError, ValueError):
            self._json(400, {"error": "volume must be a number"})
            return
        self.server.reconciler.set_local(
            on=bool(on) if on is not None else None,
            volume=parsed_volume,
        )
        self._json(200, self.server.reconciler.control_status())


def start_control_server(
    port: int, reconciler: Reconciler, *, bind: str = "127.0.0.1", token: str | None = None
) -> None:
    server = _ControlServer((bind, port), _ControlHandler)
    server.reconciler = reconciler
    server.token = token or None
    server.cors = bind not in _LOOPBACK_HOSTS
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    scope = "LAN" if server.cors else "loopback"
    print(f"[control] {scope} control surface on http://{bind}:{port}/control", flush=True)
    if server.cors and not server.token:
        print(
            "[control] WARNING: bound off-loopback with no token — anyone on the "
            "network can drive this output. Set MUSIC_CONTROL_TOKEN to require auth.",
            flush=True,
        )


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
    # A malformed env value must not raise at parse time — under systemd's
    # Restart=always that becomes a fast crash-loop that trips the start
    # limit and leaves the appliance dead instead of merely control-less.
    control_port_env: int | None = None
    raw_control_port = os.environ.get("MUSIC_CONTROL_PORT")
    if raw_control_port:
        try:
            control_port_env = int(raw_control_port)
        except ValueError:
            print(
                f"[control] ignoring non-numeric MUSIC_CONTROL_PORT={raw_control_port!r}",
                flush=True,
            )
    p.add_argument(
        "--control-port",
        type=int,
        default=control_port_env,
        help="serve the on/off+volume endpoint on this port (env MUSIC_CONTROL_PORT)",
    )
    p.add_argument(
        "--control-bind",
        default=os.environ.get("MUSIC_CONTROL_BIND", "127.0.0.1"),
        help="control-surface bind address; loopback by default. Set 0.0.0.0 to "
        "expose it on the LAN — pair with --control-token (env MUSIC_CONTROL_BIND)",
    )
    p.add_argument(
        "--control-token",
        default=os.environ.get("MUSIC_CONTROL_TOKEN"),
        help="require this token (X-Control-Token header) on control requests "
        "(env MUSIC_CONTROL_TOKEN)",
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
        start_control_server(
            args.control_port,
            reconciler,
            bind=args.control_bind,
            token=args.control_token,
        )

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
