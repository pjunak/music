/* TV-mode fallback for browsers that can't load the ES-module SPA
   (old Samsung Smart TVs, etc.). Plain ES5 — no modules, no template
   literals, no arrow functions, no optional chaining.

   Activates only when React didn't mount within 500 ms (or when ?tv
   is in the URL). Connects as a guest WebSocket and acts as a passive
   audio output: plays whatever the server says is currently playing. */
(function () {
  "use strict";

  // ---- Activation gate --------------------------------------------------

  function rootEl() { return document.getElementById("root"); }

  function forced() {
    var search = window.location.search || "";
    if (search.charAt(0) === "?") search = search.substring(1);
    var pairs = search.split("&");
    for (var i = 0; i < pairs.length; i++) {
      var key = pairs[i].split("=")[0];
      if (key === "tv") return true;
    }
    return false;
  }

  function reactMounted() {
    var r = rootEl();
    return !!(r && r.children && r.children.length > 0);
  }

  function whenReady(fn) {
    if (document.readyState === "loading") {
      document.addEventListener("DOMContentLoaded", fn);
    } else {
      fn();
    }
  }

  // ---- DOM helpers ------------------------------------------------------

  function el(tag, style, text) {
    var e = document.createElement(tag);
    if (style) e.style.cssText = style;
    if (text != null) e.appendChild(document.createTextNode(text));
    return e;
  }

  function clearNode(node) {
    while (node.firstChild) node.removeChild(node.firstChild);
  }

  function setText(node, text) {
    clearNode(node);
    node.appendChild(document.createTextNode(text == null ? "" : text));
  }

  // ---- Feature detection ------------------------------------------------

  function supportsFractionalVolume() {
    try {
      var a = document.createElement("audio");
      a.volume = 0.5;
      return a.volume > 0.4 && a.volume < 0.6;
    } catch (e) {
      return false;
    }
  }

  function clamp01(v) {
    if (v < 0) return 0;
    if (v > 1) return 1;
    return v;
  }

  function nowMs() { return new Date().getTime(); }

  // ---- Screens ----------------------------------------------------------

  var PAGE_STYLE =
    "background:#0a0a0a;color:#fff;font-family:Arial,sans-serif;" +
    "position:fixed;top:0;left:0;right:0;bottom:0;" +
    "text-align:center;overflow:hidden;";

  function renderUnsupported(reason) {
    var r = rootEl();
    clearNode(r);
    r.style.cssText = PAGE_STYLE;
    var wrap = el("div", "padding:80px 40px;");
    wrap.appendChild(el("h1", "font-size:48px;margin:0 0 24px 0;color:#ff7373;", "Browser too old to play audio"));
    wrap.appendChild(el("p", "font-size:24px;margin:0 0 24px 0;line-height:1.4;", "Control music from another device."));
    wrap.appendChild(el("p", "font-size:18px;margin:0 0 8px 0;color:#888;", "Open the same URL on a phone or laptop:"));
    wrap.appendChild(el("p", "font-size:22px;margin:0;color:#fff;", window.location.origin + "/"));
    if (reason) wrap.appendChild(el("p", "font-size:14px;margin-top:32px;color:#666;", reason));
    r.appendChild(wrap);
  }

  function renderStartScreen(onStart) {
    var r = rootEl();
    clearNode(r);
    r.style.cssText = PAGE_STYLE;
    var wrap = el("div", "padding:80px 40px;");
    wrap.appendChild(el("h1", "font-size:64px;margin:0 0 24px 0;", "TV speaker mode"));
    wrap.appendChild(el("p", "font-size:24px;margin:0 0 48px 0;color:#bbb;line-height:1.4;",
      "This screen plays whatever the controller selects. Press the button to allow audio."));
    var btn = el("button",
      "font-size:36px;padding:24px 64px;background:#4caf50;color:#fff;" +
      "border:none;cursor:pointer;border-radius:8px;font-family:inherit;",
      "Click / OK to start");
    btn.onclick = onStart;
    wrap.appendChild(btn);
    r.appendChild(wrap);
    try { btn.focus(); } catch (e) {}
  }

  function renderPlayer() {
    var r = rootEl();
    clearNode(r);
    // Re-style root as a flex container that centers the now-playing
    // card vertically. Viewport units keep everything legible on any TV
    // resolution from 720p to 4K.
    r.style.cssText =
      "background:#0a0a0a;color:#fff;font-family:Arial,sans-serif;" +
      "position:fixed;top:0;left:0;right:0;bottom:0;overflow:hidden;" +
      "display:-webkit-box;display:-webkit-flex;display:flex;" +
      "-webkit-box-orient:vertical;-webkit-box-direction:normal;" +
      "-webkit-flex-direction:column;flex-direction:column;" +
      "-webkit-box-align:center;-webkit-align-items:center;align-items:center;" +
      "-webkit-box-pack:center;-webkit-justify-content:center;justify-content:center;" +
      "padding:4vh 4vw;-webkit-box-sizing:border-box;box-sizing:border-box;";

    // Album art — square, ~36vh, soft drop shadow. The <img> sits on
    // top of a fallback "♪" glyph; onload/onerror swap their visibility
    // so a missing cover degrades to a tasteful placeholder instead of
    // a broken-image icon.
    var artWrap = el("div",
      "width:36vh;height:36vh;margin-bottom:4vh;" +
      "background:#1a1a1a;border-radius:1.2vh;overflow:hidden;" +
      "display:-webkit-box;display:-webkit-flex;display:flex;" +
      "-webkit-box-align:center;-webkit-align-items:center;align-items:center;" +
      "-webkit-box-pack:center;-webkit-justify-content:center;justify-content:center;" +
      "position:relative;" +
      "-webkit-box-shadow:0 0.8vh 3vh rgba(0,0,0,0.5);" +
      "box-shadow:0 0.8vh 3vh rgba(0,0,0,0.5);");

    var artFallback = el("div",
      "font-size:12vh;color:#333;line-height:1;", "♪");
    var artImg = document.createElement("img");
    artImg.style.cssText =
      "width:100%;height:100%;-o-object-fit:cover;object-fit:cover;" +
      "display:none;position:absolute;top:0;left:0;";
    artImg.onload = function () {
      artImg.style.display = "block";
      artFallback.style.display = "none";
    };
    artImg.onerror = function () {
      artImg.style.display = "none";
      artFallback.style.display = "";
    };
    artWrap.appendChild(artFallback);
    artWrap.appendChild(artImg);

    var title = el("h1",
      "font-size:5vh;margin:0 0 1.5vh 0;color:#fff;font-weight:600;" +
      "max-width:90vw;text-align:center;line-height:1.2;" +
      "word-wrap:break-word;-ms-word-break:break-all;word-break:break-word;",
      "—");

    var artist = el("h2",
      "font-size:3.2vh;margin:0 0 0.8vh 0;color:#bbb;font-weight:normal;" +
      "max-width:90vw;text-align:center;",
      "");

    var album = el("h3",
      "font-size:2.4vh;margin:0 0 4vh 0;color:#666;font-weight:normal;" +
      "max-width:90vw;text-align:center;",
      "");

    // Timeline row: current time · progress bar (fills middle) · total
    var timelineRow = el("div",
      "display:-webkit-box;display:-webkit-flex;display:flex;" +
      "-webkit-box-align:center;-webkit-align-items:center;align-items:center;" +
      "width:60vw;");
    var timeStyle =
      "font-size:2vh;color:#888;min-width:7ch;" +
      "font-variant-numeric:tabular-nums;-moz-font-feature-settings:'tnum';" +
      "-webkit-font-feature-settings:'tnum';font-feature-settings:'tnum';";
    var timeCurrent = el("div", timeStyle + "text-align:right;padding-right:1.5vh;", "0:00");
    var timeTotal = el("div", timeStyle + "text-align:left;padding-left:1.5vh;", "0:00");
    var progressOuter = el("div",
      "-webkit-box-flex:1;-webkit-flex:1;flex:1;" +
      "height:0.6vh;background:#222;border-radius:0.3vh;overflow:hidden;");
    var progressFill = el("div",
      "width:0%;height:100%;background:#4caf50;" +
      "-webkit-transition:width 0.2s linear;transition:width 0.2s linear;");
    progressOuter.appendChild(progressFill);
    timelineRow.appendChild(timeCurrent);
    timelineRow.appendChild(progressOuter);
    timelineRow.appendChild(timeTotal);

    r.appendChild(artWrap);
    r.appendChild(title);
    r.appendChild(artist);
    r.appendChild(album);
    r.appendChild(timelineRow);

    // Footer: device name · status · play icon. Pinned to the bottom so
    // the now-playing card stays centered regardless of footer content.
    var footer = el("div",
      "position:fixed;bottom:2.5vh;left:0;right:0;" +
      "display:-webkit-box;display:-webkit-flex;display:flex;" +
      "-webkit-box-pack:center;-webkit-justify-content:center;justify-content:center;" +
      "-webkit-box-align:center;-webkit-align-items:center;align-items:center;" +
      "font-size:1.8vh;color:#666;");
    var deviceName = el("span", "color:#888;", "");
    var sep1 = el("span", "padding:0 1.2vh;color:#333;", "·");
    var status = el("span", "color:#888;", "connecting...");
    var sep2 = el("span", "padding:0 1.2vh;color:#333;", "·");
    var playingIcon = el("span", "color:#666;", "⏸");
    footer.appendChild(deviceName);
    footer.appendChild(sep1);
    footer.appendChild(status);
    footer.appendChild(sep2);
    footer.appendChild(playingIcon);
    r.appendChild(footer);

    return {
      setStatus: function (text, color) {
        setText(status, text);
        status.style.color = color || "#888";
      },
      setTitle: function (text) { setText(title, text || "—"); },
      setArtist: function (text) { setText(artist, text || ""); },
      setAlbum: function (text) { setText(album, text || ""); },
      setAlbumArt: function (url) {
        if (url) {
          // Set src last — onload/onerror swap visibility once the
          // browser has resolved the image (cached or 404'd).
          artImg.src = url;
        } else {
          artImg.style.display = "none";
          artImg.src = "";
          artFallback.style.display = "";
        }
      },
      setProgress: function (currentMs, totalMs) {
        var c = currentMs || 0;
        var t = totalMs || 0;
        setText(timeCurrent, formatTime(c));
        setText(timeTotal, formatTime(t));
        var pct = t > 0 ? Math.min(100, Math.max(0, (c / t) * 100)) : 0;
        progressFill.style.width = pct + "%";
      },
      setDeviceName: function (name) { setText(deviceName, name || ""); },
      setPlaying: function (isPlaying) {
        // U+25B6 ▶  /  U+23F8 ⏸ — works on legacy fonts that don't
        // have the larger play-circle codepoints.
        setText(playingIcon, isPlaying ? "▶" : "⏸");
        playingIcon.style.color = isPlaying ? "#4caf50" : "#666";
      }
    };
  }

  // ---- Audio engine -----------------------------------------------------

  // 0.1 s silent WAV (1 ch, 8 kHz, 8-bit). Played within the click handler
  // so subsequent programmatic play() calls aren't blocked by autoplay
  // policy on browsers that enforce it.
  var SILENT_WAV = "data:audio/wav;base64,UklGRmQAAABXQVZFZm10IBAAAAABAAEAQB8AAEAfAAABAAgAZGF0YUAAAACAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgICA";

  function makeAudioEngine(useCrossfade) {
    var a = document.createElement("audio");
    var b = document.createElement("audio");
    a.preload = "auto";
    b.preload = "auto";
    document.body.appendChild(a);
    document.body.appendChild(b);

    var active = a;
    var inactive = b;
    var ramp = null;
    var masterVol = 1.0;

    function streamUrl(trackId) {
      return "/api/library/tracks/" + trackId + "/stream";
    }

    function stopRamp() {
      if (ramp) { clearInterval(ramp); ramp = null; }
    }

    function safePlay(audioEl) {
      try {
        var p = audioEl.play();
        if (p && typeof p["catch"] === "function") {
          p["catch"](function (err) {
            try { console.warn("[tv-mode] play() rejected:", err); } catch (e) {}
          });
        }
      } catch (e) {
        try { console.warn("[tv-mode] play() threw:", e); } catch (err) {}
      }
    }

    function prime() {
      try {
        a.src = SILENT_WAV;
        b.src = SILENT_WAV;
        safePlay(a);
        safePlay(b);
        setTimeout(function () {
          try { a.pause(); b.pause(); } catch (e) {}
        }, 50);
      } catch (e) {}
    }

    function swap(trackId, crossfadeMs, shouldPlay) {
      stopRamp();
      var url = streamUrl(trackId);
      if (!useCrossfade || crossfadeMs < 50) {
        try { active.pause(); } catch (e) {}
        active.src = url;
        active.volume = masterVol;
        if (shouldPlay) safePlay(active);
        return;
      }
      var fromEl = active;
      var toEl = inactive;
      toEl.src = url;
      toEl.volume = 0;
      if (shouldPlay) safePlay(toEl);
      var startTime = nowMs();
      ramp = setInterval(function () {
        var t = (nowMs() - startTime) / crossfadeMs;
        if (t >= 1) {
          toEl.volume = masterVol;
          try { fromEl.pause(); } catch (e) {}
          fromEl.volume = masterVol;
          try { fromEl.src = ""; } catch (e) {}
          stopRamp();
          return;
        }
        toEl.volume = clamp01(t * masterVol);
        fromEl.volume = clamp01((1 - t) * masterVol);
      }, 33);
      active = toEl;
      inactive = fromEl;
    }

    function setPlaying(shouldPlay) {
      if (shouldPlay) safePlay(active);
      else try { active.pause(); } catch (e) {}
    }

    function setVolume(v) {
      masterVol = clamp01(v);
      if (!ramp) {
        try { active.volume = masterVol; } catch (e) {}
      }
    }

    function maybeSeek(targetMs) {
      try {
        var currentMs = active.currentTime * 1000;
        if (Math.abs(currentMs - targetMs) > 1500) {
          active.currentTime = targetMs / 1000;
        }
      } catch (e) {}
    }

    function clearAudio() {
      stopRamp();
      try { a.pause(); a.src = ""; } catch (e) {}
      try { b.pause(); b.src = ""; } catch (e) {}
    }

    return {
      prime: prime,
      swap: swap,
      setPlaying: setPlaying,
      setVolume: setVolume,
      maybeSeek: maybeSeek,
      clear: clearAudio
    };
  }

  // ---- Track metadata fetch ---------------------------------------------

  function fetchTrack(trackId, cb) {
    try {
      var xhr = new XMLHttpRequest();
      xhr.open("GET", "/api/library/tracks/" + trackId, true);
      xhr.onreadystatechange = function () {
        if (xhr.readyState !== 4) return;
        if (xhr.status >= 200 && xhr.status < 300) {
          try { cb(null, JSON.parse(xhr.responseText)); }
          catch (e) { cb(e, null); }
        } else {
          cb(new Error("HTTP " + xhr.status), null);
        }
      };
      xhr.send(null);
    } catch (e) {
      cb(e, null);
    }
  }

  // ---- WebSocket client w/ reconnect ------------------------------------

  function makeWsClient(url, handlers) {
    var ws = null;
    var backoff = 1000;
    var stopped = false;
    // Attempts since the last successful onopen (or since startup). Reset
    // to 0 every time a connection opens. When it crosses GIVE_UP_AFTER
    // without ever opening, WS is declared dead for this session and
    // handlers.onGiveUp() fires so the caller can switch to a fallback
    // (HTTP polling). 2 attempts is enough signal on a TV with a
    // permanent WS-vs-cert quirk; flipping early is harmless because the
    // polling fallback works in every scenario, including transient
    // network blips during reconnection.
    var consecutiveAttemptsWithoutOpen = 0;
    var GIVE_UP_AFTER = 2;

    function connect() {
      if (stopped) return;
      consecutiveAttemptsWithoutOpen += 1;
      handlers.onStatus(
        "connecting via WebSocket... (attempt " + consecutiveAttemptsWithoutOpen + ")",
        "#888"
      );
      try {
        ws = new WebSocket(url);
      } catch (e) {
        handlers.onStatus("WebSocket constructor failed", "#ff7373");
        scheduleReconnectOrGiveUp();
        return;
      }
      ws.onopen = function () {
        consecutiveAttemptsWithoutOpen = 0;
        backoff = 1000;
        handlers.onStatus("connected via WebSocket", "#4caf50");
        handlers.onOpen(send);
      };
      ws.onmessage = function (event) {
        var msg;
        try { msg = JSON.parse(event.data); }
        catch (e) { return; }
        handlers.onMessage(msg);
      };
      ws.onerror = function () { /* onclose drives reconnect */ };
      ws.onclose = function () {
        ws = null;
        if (stopped) return;
        scheduleReconnectOrGiveUp();
      };
    }

    function scheduleReconnectOrGiveUp() {
      if (stopped) return;
      if (consecutiveAttemptsWithoutOpen >= GIVE_UP_AFTER) {
        stopped = true;
        if (typeof handlers.onGiveUp === "function") handlers.onGiveUp();
        return;
      }
      handlers.onStatus(
        "WebSocket disconnected — retrying (attempt " + (consecutiveAttemptsWithoutOpen + 1) + ")",
        "#ff7373"
      );
      var wait = backoff;
      backoff = Math.min(backoff * 2, 30000);
      setTimeout(connect, wait);
    }

    function send(action) {
      if (ws && ws.readyState === 1) {
        try { ws.send(JSON.stringify(action)); } catch (e) {}
      }
    }

    function close() {
      stopped = true;
      if (ws) { try { ws.close(); } catch (e) {} }
    }

    connect();
    return { send: send, close: close };
  }

  // ---- HTTP polling client (WebSocket fallback) -------------------------
  //
  // Fallback for browsers that loaded the page over HTTPS but can't open
  // a wss:// WebSocket — the classic case is an old smart-TV browser that
  // honors the user's "proceed anyway" cert exception for the HTML page
  // and its subresources but re-validates the cert independently for
  // WebSocket handshakes and silently rejects it. Plain XHR inherits the
  // page-level exception and works, so we poll the same PlayerState that
  // the WS would have pushed as state_snapshot / state_changed.
  //
  // Read-only: a polling TV doesn't register as a device. tv-mode audio
  // is driven entirely by state.ambient, which is global, so playback
  // works without device registration. The TV simply won't appear in the
  // controller's output list — acceptable trade-off for an emergency
  // fallback path.

  function makePollingClient(handlers) {
    var stopped = false;
    var pollTimer = null;
    var statusTimer = null;
    var lastSuccessAt = 0;
    var errorCount = 0;
    var inFlight = false;
    var hardFailed = false;
    var POLL_INTERVAL_MS = 2000;
    var ERROR_THRESHOLD = 3;

    function poll() {
      if (stopped || inFlight) return;
      inFlight = true;
      try {
        var xhr = new XMLHttpRequest();
        xhr.open("GET", "/api/sync/state", true);
        xhr.onreadystatechange = function () {
          if (xhr.readyState !== 4) return;
          inFlight = false;
          if (xhr.status >= 200 && xhr.status < 300) {
            var state = null;
            try { state = JSON.parse(xhr.responseText); }
            catch (e) { registerError(); return; }
            lastSuccessAt = nowMs();
            errorCount = 0;
            hardFailed = false;
            try { handlers.onState(state); } catch (e) {}
          } else {
            registerError();
          }
        };
        xhr.send(null);
      } catch (e) {
        inFlight = false;
        registerError();
      }
    }

    function registerError() {
      errorCount += 1;
      if (errorCount >= ERROR_THRESHOLD && !hardFailed) {
        hardFailed = true;
        if (typeof handlers.onHardFailure === "function") handlers.onHardFailure();
      }
    }

    function updateStatus() {
      if (stopped || hardFailed) return;
      if (lastSuccessAt === 0) {
        handlers.onStatus("polling — first sync...", "#ffb74d");
        return;
      }
      var since = nowMs() - lastSuccessAt;
      if (since < 3000) {
        handlers.onStatus("polling (every 2s)", "#4caf50");
      } else if (since < 15000) {
        handlers.onStatus(
          "polling — last sync " + Math.floor(since / 1000) + "s ago",
          "#ffb74d"
        );
      } else {
        handlers.onStatus(
          "polling — last sync " + Math.floor(since / 1000) + "s ago",
          "#ff7373"
        );
      }
    }

    function close() {
      stopped = true;
      if (pollTimer) { clearInterval(pollTimer); pollTimer = null; }
      if (statusTimer) { clearInterval(statusTimer); statusTimer = null; }
    }

    poll(); // kick off immediately so the user sees data within ~1s
    pollTimer = setInterval(poll, POLL_INTERVAL_MS);
    statusTimer = setInterval(updateStatus, 1000);

    return { close: close };
  }

  // ---- Main orchestration -----------------------------------------------

  function makeShortId() {
    var alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
    var out = "";
    for (var i = 0; i < 5; i++) {
      out += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
    }
    return out;
  }

  function getQueryParam(key) {
    var search = window.location.search || "";
    if (search.charAt(0) === "?") search = search.substring(1);
    var pairs = search.split("&");
    for (var i = 0; i < pairs.length; i++) {
      var kv = pairs[i].split("=");
      if (kv[0] === key) {
        try { return decodeURIComponent((kv[1] || "").replace(/\+/g, " ")); }
        catch (e) { return kv[1] || ""; }
      }
    }
    return null;
  }

  // Device name resolution:
  //   1. ?name=... URL param (one-time setup — also persists to localStorage)
  //   2. localStorage from a previous visit
  //   3. generated default "Old TV (xxxxx)" — randomized each load
  // Bookmark `https://music.example/?tv&name=Living%20Room` once to claim
  // a stable label that survives subsequent reloads.
  function getDeviceName() {
    var fromUrl = getQueryParam("name");
    if (fromUrl) {
      try { window.localStorage && localStorage.setItem("tv-mode.name", fromUrl); }
      catch (e) {}
      return fromUrl;
    }
    try {
      var stored = window.localStorage && localStorage.getItem("tv-mode.name");
      if (stored) return stored;
    } catch (e) {}
    return "Old TV (" + makeShortId() + ")";
  }

  function formatTime(ms) {
    if (!ms || ms < 0 || isNaN(ms)) return "0:00";
    var totalSec = Math.floor(ms / 1000);
    var min = Math.floor(totalSec / 60);
    var sec = totalSec % 60;
    return min + ":" + (sec < 10 ? "0" : "") + sec;
  }

  function start() {
    var crossfadeOk = supportsFractionalVolume();
    try { console.log("[tv-mode] crossfade supported:", crossfadeOk); } catch (e) {}
    var ui = renderPlayer();
    var engine = makeAudioEngine(crossfadeOk);
    engine.prime();

    var lastTrackId = null;
    var lastIsPlaying = false;
    var deviceLabel = getDeviceName();
    ui.setDeviceName(deviceLabel);
    // Server-assigned device id. Captured from the first state_snapshot
    // when connected via WebSocket; stays null in polling mode (no
    // registration possible). isThisDeviceActive() treats null as
    // "always play" so the polling fallback keeps acting as a passive
    // speaker — the only mode where it can do anything useful, since
    // pollers can't appear in active_output_device_ids.
    var myDeviceId = null;
    // Timeline state. Server pushes position_ms in every state broadcast
    // (~every 2 s); we interpolate between pushes locally so the
    // progress bar moves smoothly. currentTrackLengthMs comes from
    // /api/library/tracks/<id> via fetchTrack on each track change.
    var currentTrackLengthMs = 0;
    var serverPositionMs = 0;
    var serverPositionTimestamp = nowMs();

    function isThisDeviceActive(state) {
      if (myDeviceId === null) return true;
      var outputs = state.active_output_device_ids || [];
      for (var i = 0; i < outputs.length; i++) {
        if (outputs[i] === myDeviceId) return true;
      }
      return false;
    }

    function applyState(state) {
      if (!state) return;
      var amb = state.ambient || {};
      var trackId = (amb.current_track_id == null) ? null : amb.current_track_id;
      var posMs = amb.position_ms || 0;
      // Gate playback on this device being a selected output (when
      // registered). The TV otherwise behaves as a passive subscriber
      // that wouldn't show up in the controller UI's output picker —
      // see audio_output capability in the WS register call below.
      var isPlaying = !!state.is_playing && isThisDeviceActive(state);
      var vol = (typeof state.volume === "number") ? state.volume : 1;
      var crossfadeMs = state.crossfade_ms || 0;

      engine.setVolume(vol);
      // Capture latest server position for the progress interpolator
      // (runs on its own 250 ms timer below).
      serverPositionMs = posMs;
      serverPositionTimestamp = nowMs();

      if (trackId !== lastTrackId) {
        if (trackId == null) {
          engine.clear();
          ui.setTitle("");
          ui.setArtist("");
          ui.setAlbum("");
          ui.setAlbumArt(null);
          currentTrackLengthMs = 0;
        } else {
          engine.swap(trackId, crossfadeMs, isPlaying);
          ui.setTitle("Track " + trackId);
          ui.setArtist("");
          ui.setAlbum("");
          // Backend serves /cover with the right MIME or 404s — the
            // <img> onerror handler falls back to the placeholder glyph.
          ui.setAlbumArt("/api/library/tracks/" + trackId + "/cover");
          currentTrackLengthMs = 0;
          fetchTrack(trackId, function (err, t) {
            if (err || !t) return;
            ui.setTitle(t.display_title || t.title || ("Track " + trackId));
            ui.setArtist(t.artist || "");
            ui.setAlbum(t.album || "");
            if (typeof t.length_s === "number" && t.length_s > 0) {
              currentTrackLengthMs = Math.floor(t.length_s * 1000);
            }
          });
        }
        lastTrackId = trackId;
      } else if (trackId != null) {
        engine.maybeSeek(posMs);
      }

      if (isPlaying !== lastIsPlaying) {
        engine.setPlaying(isPlaying);
        lastIsPlaying = isPlaying;
      }
      ui.setPlaying(isPlaying);
    }

    // Progress bar interpolator. Server pushes position only on state
    // changes (~every 2 s + at every mutation), so the timeline would
    // otherwise jump in 2 s steps. Between pushes we extrapolate from
    // the last server position using elapsed wall-clock time, clipped
    // to the track length. Runs at 4 Hz — visually smooth, negligible
    // CPU even on a 2015 TV browser.
    setInterval(function () {
      if (!lastTrackId) {
        ui.setProgress(0, 0);
        return;
      }
      var displayMs = serverPositionMs;
      if (lastIsPlaying) {
        displayMs += (nowMs() - serverPositionTimestamp);
      }
      if (currentTrackLengthMs > 0 && displayMs > currentTrackLengthMs) {
        displayMs = currentTrackLengthMs;
      }
      ui.setProgress(displayMs, currentTrackLengthMs);
    }, 250);

    var wsScheme = (window.location.protocol === "https:") ? "wss://" : "ws://";
    var wsUrl = wsScheme + window.location.host + "/api/ws";

    // Fallback path: when makeWsClient gives up (e.g. the TV browser
    // refuses the wss:// cert even though the user accepted it for the
    // page), switch to polling /api/sync/state. The 150ms delay lets the
    // operator see the "switching..." status transition on the TV screen.
    function startPolling() {
      // Drop the WS-assigned device id — polling can't keep a registered
      // device alive, so the server has already pruned it. Clearing this
      // makes isThisDeviceActive() fall back to "always play" (passive
      // speaker mode), the only behaviour that's useful when we can't
      // be selected as an output.
      myDeviceId = null;
      ui.setStatus("WebSocket blocked — switching to HTTP polling...", "#ffb74d");
      setTimeout(function () {
        makePollingClient({
          onState: applyState,
          onStatus: ui.setStatus,
          onHardFailure: function () {
            ui.setStatus("Cannot reach server — check network / cert", "#ff7373");
          }
        });
      }, 150);
    }

    makeWsClient(wsUrl, {
      onStatus: ui.setStatus,
      onOpen: function (send) {
        // Advertise audio_output so the controller's UI lists this TV
        // in its output picker. Without it the device registers but is
        // invisible in the picker, and the user can't toggle it.
        send({ type: "register", name: deviceLabel, capabilities: ["audio_output"] });
      },
      onMessage: function (msg) {
        if (!msg || !msg.type) return;
        if (msg.type === "state_snapshot") {
          // Server's assigned id for this connection — used by
          // isThisDeviceActive() to check active_output_device_ids.
          if (typeof msg.your_device_id === "string") {
            myDeviceId = msg.your_device_id;
          }
          applyState(msg.state);
        } else if (msg.type === "state_changed") {
          applyState(msg.state);
        }
      },
      onGiveUp: startPolling
    });
  }

  // ---- Bootstrap --------------------------------------------------------

  // ES modules were added to Chrome v61, Firefox 60, Safari 11. Browsers
  // missing this support can't load the SPA's module entry script and
  // need tv-mode immediately, with no wait. Modern browsers get up to
  // 4 s to mount React before tv-mode concludes the SPA is broken and
  // takes over — fixes the F5 false-positive where a slow page reload
  // got kidnapped after 500 ms.
  function browserSupportsEsModules() {
    try { return "noModule" in document.createElement("script"); }
    catch (e) { return false; }
  }

  function activateTvMode() {
    if (typeof WebSocket === "undefined") {
      renderUnsupported("WebSocket support is required.");
      return;
    }
    renderStartScreen(function () { start(); });
  }

  function pollForSpa(timeoutMs) {
    var startTime = nowMs();
    (function tick() {
      if (reactMounted()) return;            // SPA is up, stay out of the way
      if (nowMs() - startTime >= timeoutMs) {
        activateTvMode();
        return;
      }
      setTimeout(tick, 100);
    })();
  }

  whenReady(function () {
    if (forced())                       { activateTvMode(); return; }
    if (!browserSupportsEsModules())    { activateTvMode(); return; }
    pollForSpa(4000);
  });
})();
