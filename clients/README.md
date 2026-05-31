# Output clients — make anything an audio output

This player is built so that **being an audio output is decoupled from running the
web app**. The server is the single source of truth for *what should be playing right
now* (current track, position, play/pause, volume); any client just **reconciles** to
that and renders the audio however it likes.

That means another project, a web page, or a tiny headless box plugged into a speaker
can act as an output **with no special server setup and no login** — the protocol below
already works today against an unmodified server.

- Want a ready-made appliance? Use the reference client in [`headless/`](headless/) — a
  single Python file you run on any Linux box (a Pi, an old x86 stick, the dnd-table
  machine) to turn a speaker into an output.
- Want to bake on/off + volume into your own project? Implement the ~30-line protocol
  below. A copy-paste browser example is included.

---

## The model in one paragraph

The server holds a canonical `PlayerState`. A client connects, learns the current state,
and on every change decides: *should I be producing sound, and if so, which track at what
position?* It then plays `GET /api/library/tracks/{id}/stream`. There is **no per-client
audio rendered on the server** — the server only streams raw track files and broadcasts
state. Effects/crossfade/EQ are a *browser-engine* feature; a simple client just plays the
current track.

## Two ways to follow state

| | Transport | Use when |
|---|---|---|
| **WebSocket** (recommended) | `ws(s)://<host>/api/ws` — server pushes on every change | You want instant reaction to play/pause/skip |
| **HTTP polling** | `GET /api/sync/state` every ~1–2s — returns the full `PlayerState` | Your environment can't open a WebSocket (some old TV browsers, constrained MCUs) |

Both return the **same** `PlayerState` shape. Both are reachable **without authentication**
(guest access) — see [Auth](#auth).

## The WebSocket handshake

1. Connect to `ws(s)://<host>/api/ws` (`wss` if the site is `https`).
2. The server immediately sends a **`state_snapshot`**:
   ```json
   { "type": "state_snapshot", "your_device_id": "dev-AbC123", "state": { ...PlayerState } }
   ```
   Keep `your_device_id` — it's how you recognise yourself in `active_output_device_ids`.
3. Send a **`register`** so you appear in the operator's Console "Outputs" picker and
   (with `audio_output`) receive SFX events:
   ```json
   { "type": "register", "name": "Living-room speaker", "capabilities": ["audio_output"] }
   ```
4. From then on the server pushes a **`state_changed`** on every change:
   ```json
   { "type": "state_changed", "state": { ...PlayerState } }
   ```

No heartbeat/ping is required at the application level (the WS library's protocol-level
ping/pong is enough). Reconnect on close and repeat from step 2.

## The fields you actually need

`PlayerState` is large; an output only cares about a handful (see
[`backend/app/sync/protocol.py`](../backend/app/sync/protocol.py) for the full schema):

| Field | Meaning |
|---|---|
| `is_playing` | Whether the **ambient** lane should be playing |
| `volume` | Master volume `0.0–1.0` (server-wide; informational for a guest) |
| `ambient.current_track_id` | The track id to play (or `null` = nothing) |
| `ambient.position_ms` | Where in that track playback should be |
| `ambient.queue` / `ambient.history` | What's next / previous (only needed if you show a queue) |
| `interrupt` | `null`, or an object `{ current_track_id, position_ms, … }` that **takes over** while present (alerts/stingers) |
| `active_output_device_ids` | The device ids the operator has switched **on** |

### Deciding what to play

```
active track  = interrupt ? interrupt.current_track_id : ambient.current_track_id
playing       = interrupt ? true : is_playing
position      = interrupt ? interrupt.position_ms : ambient.position_ms
am I "on"?    = (your_device_id is in active_output_device_ids)   ← server/Console-driven
                OR a local on/off you control yourself             ← see "On/off" below
```

If you're "on" and `playing` and `active track` isn't `null`: ensure your player is loaded
to `stream_url(active track)` and (on a track change or a >~1.5s jump) seek to `position`.
Otherwise pause. The server only re-stamps `position_ms` on real actions (play/seek/skip),
so don't re-seek on every frame — only when the track id changes or the position genuinely
jumps.

## Media URLs

| What | URL | Auth |
|---|---|---|
| Audio stream | `GET /api/library/tracks/{id}/stream` | guest OK |
| Cover art | `GET /api/library/tracks/{id}/cover` | guest OK |
| Track metadata (title/artist/length…) | `GET /api/library/tracks/{id}` | guest OK |
| SFX clip | `GET /api/sfx/file?path=<rel>` | guest OK (must be a path referenced by a loaded soundboard) |

## SFX events

If you registered with `audio_output`, the server also pushes fire-and-forget sound effects:
```json
{ "type": "sfx_fired", "soundboard_id": "tavern", "item_path": "dnd/door.ogg", "volume": 0.8 }
```
Play `GET /api/sfx/file?path=<item_path>` once, at `volume × your-local-volume`, layered over
the music. These are **not** part of `PlayerState` — they're transient, so just play and forget.

## Auth

The whole output protocol works **as a guest** — no cookie, no token. A guest socket can:

- receive `state_snapshot` / `state_changed` / `sfx_fired`,
- `register` (so it shows up as a device and can be toggled from the Console),
- stream tracks, covers, metadata, and referenced SFX.

A guest **cannot** mutate server state. For an output that's exactly right — you *follow*
state, you don't drive it. The narrow consequences:

- **On/off via the Console doesn't persist.** Each reconnect mints a fresh `your_device_id`,
  so if you rely on the operator toggling you on in the Console, you'll need re-toggling after
  a reboot. **Recommendation: keep a *local* on/off** (default on) so the box just plays — see
  the reference client's `--respect-console` flag for the opposite behaviour.
- **No position reporting back to the server.** Playback is fine (you follow `position_ms`);
  only the server's *authoritative* scrub position won't be corrected by you. For ambient
  background this is unnoticeable.
- **No master volume / transport from the client.** Use a **local** volume (how loud *this*
  speaker is). The reference client and the embed example both do this.

If you later need persistent Console on/off, master-volume/transport from a third-party page,
or position reporting **without** a browser login, that's the optional token layer described in
the plan (`docs`/Phase 2) — not required for any of the above.

---

## Recipe A — embed into another web project (≈30 lines)

Drop this into any page. It plays the current track through a plain `<audio>` element and
gives you an on/off button + a volume slider. (Plain `<audio>` means no effects/crossfade —
that's the browser *engine* feature; this is the simple output.)

```html
<button id="toggle">Music: OFF</button>
<input id="vol" type="range" min="0" max="1" step="0.01" value="1" />
<audio id="out" hidden></audio>
<script>
  const SERVER = "https://music.example";              // ← the player's origin
  const el = document.getElementById("out");
  const btn = document.getElementById("toggle");
  const vol = document.getElementById("vol");
  let on = false, state = null, loadedId = null;

  const host = new URL(SERVER).host;
  const wsProto = SERVER.startsWith("https") ? "wss" : "ws";
  const streamUrl = (id) => `${SERVER}/api/library/tracks/${id}/stream`;
  const activeTrack = (s) =>
    s.interrupt?.current_track_id ?? s.ambient.current_track_id ?? null;

  function apply() {
    if (!state) return;
    const id = activeTrack(state);
    const playing = state.interrupt ? true : state.is_playing;
    if (!on || id === null || !playing) { el.pause(); return; }
    if (id !== loadedId) { el.src = streamUrl(id); loadedId = id; }
    el.play().catch(() => {});   // first play needs a user gesture — the button click is it
  }

  const ws = new WebSocket(`${wsProto}://${host}/api/ws`);
  ws.onmessage = (e) => {
    const m = JSON.parse(e.data);
    if (m.type === "state_snapshot") {
      state = m.state;
      ws.send(JSON.stringify(
        { type: "register", name: "Web embed", capabilities: ["audio_output"] }));
      apply();
    } else if (m.type === "state_changed") { state = m.state; apply(); }
  };
  btn.onclick = () => { on = !on; btn.textContent = `Music: ${on ? "ON" : "OFF"}`; apply(); };
  vol.oninput = () => { el.volume = Number(vol.value); };   // local volume for this page only
</script>
```

Cross-origin is fine: the `<audio>` element streams media from the player's origin without
CORS restrictions, and browser WebSocket connections aren't subject to CORS. The only gotcha
is the browser autoplay policy — audio won't start until a user gesture, which the on/off
button provides.

## Recipe B — any language, no WebSocket

Poll the snapshot and drive any media player:

```sh
# what should be playing right now?
curl -s https://music.example/api/sync/state | jq '{playing:.is_playing, track:.ambient.current_track_id, pos:.ambient.position_ms}'

# stream that track (e.g. into mpv / ffmpeg / vlc)
mpv "https://music.example/api/library/tracks/42/stream"
```

Or watch the live socket with [`websocat`](https://github.com/vi/websocat):

```sh
websocat wss://music.example/api/ws
# → {"type":"state_snapshot","your_device_id":"dev-…","state":{…}}
# then paste:  {"type":"register","name":"cli","capabilities":["audio_output"]}
```

The [reference Python client](headless/) is Recipe B done properly (reconnect, seek
handling, SFX, local on/off + volume, optional LAN control endpoint).

---

## dnd-table integration

The [dnd-table](https://github.com/pjunak/dnd-table) rig's TV display is a native
GStreamer/pyglet kiosk (not a browser), so the audio output there is the **headless client**
on that same machine, and the on/off + volume control lives in the existing `control.html`:

1. **Audio:** run [`headless/music_output.py`](headless/) on the dnd-table box as a systemd
   service (see its README), pointed at the player with `MUSIC_CONTROL_PORT` set so it exposes
   a small LAN control endpoint.
2. **Control:** add a "Music" card to `templates/control.html` (vanilla JS) that talks to the
   appliance's `/control` endpoint **on the LAN** — no player credential needed:
   ```js
   const OUT = "http://dnd-table.local:8731";   // the appliance's MUSIC_CONTROL_PORT
   async function refresh() {
     const s = await (await fetch(`${OUT}/control`)).json();   // {on, volume, is_playing, title, artist}
     // …render on/off state + now-playing…
   }
   const setOn  = (on)  => fetch(`${OUT}/control`, {method:"POST", body: JSON.stringify({on})});
   const setVol = (v)   => fetch(`${OUT}/control`, {method:"POST", body: JSON.stringify({volume:v})});
   ```

This keeps the player server untouched and the dnd-table repo's only change a small card in
its own control panel. For full-fidelity effects/crossfade on that box (rather than plain
ambient playback), the alternative is to run a kiosk browser pointed at the player's web app
instead of the headless client — heavier, and not required for music to come out of the
speakers.
