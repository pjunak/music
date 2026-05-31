# Headless output appliance

A single Python file that turns a Linux box with a speaker into an audio output for the
music player. Plug a Pi / old x86 stick / the dnd-table machine into a speaker, run this,
and it plays whatever the DM has going — following play/pause/skip/track changes from the
server. No login and no server changes (it connects as a guest; see
[the protocol guide](../README.md)).

It's a deliberately *dumb* player — ambient music + soundboard SFX, no crossfade/EQ/scene
effects (those are a browser-engine feature). That's the right trade-off for a
leave-it-on-a-shelf speaker box.

## Install (Debian/Ubuntu)

```sh
# 1. libmpv + python
sudo apt update && sudo apt install -y python3 python3-pip mpv libmpv2   # older: libmpv1

# 2. the client + its python deps
sudo mkdir -p /opt/music-output
sudo cp music_output.py /opt/music-output/
sudo pip3 install -r requirements.txt        # or: pip3 install --user websocket-client python-mpv
```

## Run it once to test

```sh
MUSIC_SERVER_URL=http://192.168.1.50:8000 python3 /opt/music-output/music_output.py
```

You should see it connect and register; start playback from the player and audio comes out
of this box. It also shows up in the player Console's **Outputs** picker as the name you gave
it (default: the hostname).

## Run it forever (systemd)

```sh
# config
sudo tee /etc/music-output.env >/dev/null <<'EOF'
MUSIC_SERVER_URL=http://192.168.1.50:8000
MUSIC_OUTPUT_NAME=Living-room speaker
# optional: expose a tiny LAN on/off + volume endpoint (see "control surface" below)
MUSIC_CONTROL_PORT=8731
EOF

# service
sudo cp music-output.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now music-output
journalctl -u music-output -f          # watch it
```

## Options

All flags have an environment-variable equivalent (handy for the systemd env file):

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--server URL` | `MUSIC_SERVER_URL` | — (required) | The player's base URL |
| `--name NAME` | `MUSIC_OUTPUT_NAME` | hostname | Name shown in the Console |
| `--control-port N` | `MUSIC_CONTROL_PORT` | off | Serve the LAN control endpoint |
| `--volume V` | `MUSIC_VOLUME` | `1.0` | Initial local volume `0..1` |
| `--start-off` | `MUSIC_START_ON=0` | off (boots playing) | Boot muted |
| `--respect-console` | — | off | Only play when switched on in the Console (instead of the default local on/off) |
| `--no-sfx` | — | off (SFX on) | Ignore soundboard SFX events |

### On/off model

By default the box has its **own** on/off (on at boot) and plays whenever the server is
playing — reliable for an appliance, because guest connections get a fresh device id each
reconnect, so *Console*-driven on/off wouldn't survive a reboot. Pass `--respect-console`
if you'd rather the operator switch this output on/off from the player's Outputs picker.

### Local control surface (for the dnd-table panel, or anything on the LAN)

Set `MUSIC_CONTROL_PORT` and the appliance serves a tiny HTTP endpoint:

```
GET  /control   → {"on":true,"volume":1.0,"is_playing":true,"track_id":42,"title":"…","artist":"…"}
POST /control   {"on":false}            → toggle this speaker
POST /control   {"volume":0.4}          → set this speaker's volume (0..1)
```

It sends permissive CORS headers so a browser page on another LAN origin (e.g. the dnd-table
`control.html`) can drive it directly — no player credential involved. Treat it as LAN-only;
don't expose this port to the internet.

Example "Music" card for the dnd-table control panel:

```js
const OUT = "http://dnd-table.local:8731";
async function refresh() {
  const s = await (await fetch(`${OUT}/control`)).json();
  musicBtn.textContent = `Music: ${s.on ? "ON" : "OFF"}`;
  nowPlaying.textContent = s.title ? `${s.title} — ${s.artist ?? ""}` : "";
  volSlider.value = s.volume;
}
musicBtn.onclick = () => fetch(`${OUT}/control`,
  {method:"POST", body: JSON.stringify({on: !lastOn})}).then(refresh);
volSlider.oninput = (e) => fetch(`${OUT}/control`,
  {method:"POST", body: JSON.stringify({volume: +e.target.value})});
setInterval(refresh, 2000);
```

## Want full-fidelity effects on this box instead?

This client plays plain ambient + SFX. If you specifically want crossfades, EQ presets, and
scene colouring on this output, run a kiosk browser pointed at the player's web app instead of
this client (it's heavier, and not needed just to get music out of the speakers). The headless
client is the right choice for tiny/always-on appliances.
