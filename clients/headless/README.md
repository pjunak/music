# Headless output appliance

A small Rust binary that turns a Linux box with a speaker into an audio output for the
music player. Plug a Pi / old x86 stick / the dnd-table machine into a speaker, run it,
and it plays whatever the DM has going — following play/pause/skip/track changes from the
server. No login and no server changes are needed (it connects as a guest; see
[the protocol guide](../README.md)).

It's a deliberately *dumb* player — ambient music + soundboard SFX, no crossfade/EQ/effect
colouring (those are a browser-engine feature). That's the right trade-off for a
leave-it-on-a-shelf speaker box.

## Tested Linux download

Each current `main` commit that passes the release checks publishes a
[GitHub release](https://github.com/pjunak/music/releases/latest) containing
`music-output-linux-x86_64.tar.gz`, `SHA256SUMS` and `REVISION`. The download is
for **x86-64 Debian 13 / Ubuntu 24.04 or newer** (glibc 2.39 or newer). It needs
mpv and trusted system CA certificates, but no Rust compiler or GitHub token.
Other architectures, including ARM Raspberry Pi boards, currently build from source.

A release is named `music-output-<full commit ID>`. This identifies the tested
source without waiting for a version-number change. CI reuses the binary it tested;
reruns never replace already published bytes. Publishing makes an update available
and does not install it on any device.

### D&D table: install, update and rollback

Use the table's [Music installer](https://github.com/pjunak/dnd-table#music-output):

```bash
# From the updated dnd-table source folder. The same command installs or updates.
bash install-music.sh
# Restore the previous player and service if necessary.
bash install-music.sh --rollback
```

This checks the package checksum and source revision before stopping the old player.
It preserves `/etc/music-output.env`, the existing `dndtable` account and its stable
client ID. The old binary and unit are saved; a startup failure restores them.
Legacy runtime files stay available for rollback until the physical check below
passes. The display/kiosk installation and normal table Update button stay separate.

### Other Linux speaker boxes

Download and verify the public package before installing it:

```bash
mkdir -p music-output-download && cd music-output-download
# Resolve latest once so both files come from the same source revision.
release=$(curl --fail --silent --show-error --location --output /dev/null --write-out '%{url_effective}' https://github.com/pjunak/music/releases/latest)
base="${release/\/tag\//\/download\/}"
curl --fail --show-error --location --remote-name "$base/music-output-linux-x86_64.tar.gz"
curl --fail --show-error --location --remote-name "$base/SHA256SUMS"
sha256sum -c SHA256SUMS
tar -xzf music-output-linux-x86_64.tar.gz
sudo apt update && sudo apt install -y mpv ca-certificates
sudo install -D -m 0755 music-output /opt/music-output/music-output
```

For a fresh systemd installation, create `/etc/music-output.env` containing:

```ini
MUSIC_SERVER_URL=http://your-music-server:8000
MUSIC_OUTPUT_NAME=Living-room speaker
# Optional loopback control endpoint:
MUSIC_CONTROL_PORT=8731
```

Then install the included service and start it:

```bash
sudo cp music-output.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now music-output
journalctl -u music-output -f
```

The generic unit uses a dynamic unprivileged user and stores its identity in
`/var/lib/music-output`. When migrating an existing speaker, retain its service
account and state directory instead of switching to this fresh-install unit.
The table installer handles that migration for the `dndtable` account.

### Build from source

```bash
cargo build --locked --release -p music-output
MUSIC_SERVER_URL=http://your-music-server:8000 target/release/music-output
```

The device appears in the Console's Outputs picker. Saving it in Settings → Devices
is optional; output by default makes the server activate its stable identity on reconnect.

## Options

Most options also have an environment-variable equivalent (handy for the systemd env file):

| Flag | Env | Default | Meaning |
|---|---|---|---|
| `--server URL` | `MUSIC_SERVER_URL` | — (required) | The player's base URL |
| `--name NAME` | `MUSIC_OUTPUT_NAME` | hostname | Name shown in the Console |
| `--client-id ID` | `MUSIC_CLIENT_ID` | persisted generated id | Override the stable identity |
| — | `MUSIC_STATE_DIR` | `~/.config/music-output` | Directory for the generated client id |
| `--control-port N` | `MUSIC_CONTROL_PORT` | off | Serve the on/off+volume control endpoint |
| `--control-bind ADDR` | `MUSIC_CONTROL_BIND` | `127.0.0.1` | Control bind address; `0.0.0.0` to expose on the LAN |
| `--control-token TOKEN` | `MUSIC_CONTROL_TOKEN` | — | Require this token (`X-Control-Token`) on control requests |
| `--volume V` | `MUSIC_VOLUME` | `1.0` | Initial local volume `0..1` |
| `--start-off` | `MUSIC_START_ON=0` | off (boots playing) | Boot muted |
| `--respect-console` | — | off | Only play when switched on in the Console (instead of the default local on/off) |
| `--no-sfx` | — | off (SFX on) | Ignore soundboard SFX events |
| `--mpv PATH` | `MUSIC_MPV` | `mpv` | mpv executable to supervise for both audio lanes |

### On/off model

By default the box has its **own** on/off (on at boot) and plays whenever the server is
playing. Its generated client id is persisted under
`$MUSIC_STATE_DIR/client-id` (default `~/.config/music-output/client-id`), so server volume and
the optional output-by-default setting remain attached across reconnects. Pass
`--respect-console` if the appliance should also require live activation in the Console.

### Local control surface (for the dnd-table panel, or anything on the LAN)

Set `MUSIC_CONTROL_PORT` and the appliance serves a tiny HTTP endpoint:

```
GET  /control   → {"on":true,"volume":1.0,"is_playing":true,"track_id":42,"title":"…","artist":"…"}
POST /control   {"on":false}            → toggle this speaker
POST /control   {"volume":0.4}          → set this speaker's volume (0..1)
```

**It binds to loopback (`127.0.0.1`) by default** — reachable only from the box itself. To let
another device drive it (e.g. the dnd-table `control.html` on a different LAN origin), set
`--control-bind 0.0.0.0`. When bound off-loopback the endpoint emits permissive CORS **and**
you should set `--control-token`: pass the same value in an `X-Control-Token` header on each
request. Without a token, anything on the network can toggle/mute this speaker (the appliance
prints a warning at startup). Never expose this port to the internet.

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

This client supervises separate ambient and SFX mpv subprocesses through bounded Unix-socket
JSON IPC. If either subprocess dies, the appliance exits so systemd restarts the complete,
stable-ID client cleanly. It plays plain ambient + SFX. If you specifically want crossfades and EQ-preset
colouring on this output, run a kiosk browser pointed at the player's web app instead of
this client (it's heavier, and not needed just to get music out of the speakers). The headless
client is the right choice for tiny/always-on appliances.

## Rust cutover acceptance on the intended speaker

Run this final check on the actual Linux output device, not through a development-machine audio
substitute. Record the rewrite commit, OS, mpv version, sound device, and pass/fail result without
copying credentials or private media paths into the repository.

1. Install the Rust binary and service, preserving the existing stable client ID, name, output
   designation, server URL, and local-control token.
2. Confirm the device appears once in the web UI, remains selected across an appliance restart, and
   reconnects after a temporary network interruption without creating a duplicate remembered device.
3. Play one normal track and exercise pause/resume, seek, skip, server master volume, per-device
   volume, local mute/unmute, and a server-side output enable/disable transition. Confirm position
   reports remain monotonic and an epoch-changing seek/restart is heard at the new position.
4. Trigger overlapping SFX while ambient audio is playing. Confirm the lanes remain independent and
   both respect their effective volume controls.
5. Terminate either owned mpv child. The appliance must exit, systemd must restart the complete
   service, the same stable client must reconnect, and playback must recover without an orphan mpv
   process.
6. Leave playback running for at least thirty minutes. Pass only if there are no repeated reconnects,
   unbounded log growth, audible stalls, or device-state divergence between the web UI and appliance.

This physical check is the final headless-output acceptance gate; the Unix fake-mpv suite proves
process and IPC semantics but cannot prove the selected ALSA/PipeWire route or speaker hardware.
