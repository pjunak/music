/* Compatibility-mode fallback for browsers that can't load the ES-module
   SPA (old Samsung Smart TVs, etc.). Plain ES5 — no modules, no template
   literals, no arrow functions, no optional chaining.

   Loaded as <script nomodule> (index.html): a module-capable browser
   skips it entirely, a module-incapable one runs it instead. That
   capability split IS the "is compatibility mode needed?" decision — so
   there's no timer and no race with React. main.tsx also injects it on
   demand for the ?compat preview (legacy alias: ?tv). In both cases the
   SPA won't render in this document, so activation is immediate.

   Registers a stable client_id (exactly like the SPA) so the operator can
   pick this device as an output in the Speakers popover, set its per-device
   volume, and designate it output-by-default; it then plays whatever the
   controller routes to it. If the wss:// WebSocket can't be opened (the
   classic old-TV cert quirk — see makePollingClient) it falls back to
   read-only HTTP polling and acts as a passive always-on speaker. */
(function () {
  "use strict";

  // ---- Activation gate --------------------------------------------------

  function rootEl() { return document.getElementById("root"); }

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
    wrap.appendChild(el("h1", "font-size:64px;margin:0 0 24px 0;", "Compatibility mode"));
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
            try { console.warn("[compat-mode] play() rejected:", err); } catch (e) {}
          });
        }
      } catch (e) {
        try { console.warn("[compat-mode] play() threw:", e); } catch (err) {}
      }
    }

    // Pending-seek plumbing. Old TV browsers throw INVALID_STATE_ERR when
    // currentTime is set before the element has metadata, and the old
    // try/catch just swallowed that — the seek was silently lost. Instead,
    // park the target on the element and apply it on `loadedmetadata`. One
    // persistent listener per element; the latest request wins (a new src
    // load clears any stale target).
    function attachPendingSeek(el) {
      el.addEventListener("loadedmetadata", function () {
        var t = el.__pendingSeekS;
        el.__pendingSeekS = null;
        if (t != null) {
          try { el.currentTime = t; } catch (e) {}
        }
      });
    }
    attachPendingSeek(a);
    attachPendingSeek(b);

    function requestSeek(el, targetMs) {
      var t = targetMs / 1000;
      if (el.readyState >= 1) {  // HAVE_METADATA — safe to seek now
        el.__pendingSeekS = null;
        try { el.currentTime = t; } catch (e) { el.__pendingSeekS = t; }
      } else {
        el.__pendingSeekS = t;
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

    // Load `trackId` into the player, optionally starting at `startMs`
    // (a mid-track join or an interrupt (re)join; 0/absent = from the top).
    function swap(trackId, crossfadeMs, shouldPlay, startMs) {
      stopRamp();
      var url = streamUrl(trackId);
      if (!useCrossfade || crossfadeMs < 50) {
        try { active.pause(); } catch (e) {}
        active.__pendingSeekS = null;
        active.src = url;
        active.volume = masterVol;
        if (startMs && startMs > 500) requestSeek(active, startMs);
        if (shouldPlay) safePlay(active);
        return;
      }
      var fromEl = active;
      var toEl = inactive;
      toEl.__pendingSeekS = null;
      toEl.src = url;
      toEl.volume = 0;
      if (startMs && startMs > 500) requestSeek(toEl, startMs);
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

    // Unconditional seek. WHETHER to seek is decided entirely by applyState's
    // position_epoch gate (the server's explicit "a seek happened" signal);
    // re-gating here against the element clock would open a dead-band that
    // silently drops small honest seeks the outer gate already admitted.
    function seek(targetMs) {
      requestSeek(active, targetMs);
    }

    // The active element's clock, for position reports. null when unknown.
    function getPositionMs() {
      try {
        var t = active.currentTime;
        if (typeof t === "number" && !isNaN(t)) return Math.floor(t * 1000);
      } catch (e) {}
      return null;
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
      seek: seek,
      getPositionMs: getPositionMs,
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
    // Connection attempts since startup that never reached onopen (reset to 0
    // on every successful open). We fall back to HTTP polling ONLY if the
    // socket has never opened even once — that's the genuine permanent signal
    // (an old TV whose wss:// handshake is cert-rejected; see
    // makePollingClient). Once the socket HAS opened we know WS works on this
    // TV, so a later drop is a transient blip: keep retrying WS with backoff
    // forever rather than demoting (one-way, until a physical reload) to
    // read-only polling and silently losing the TV's output selectability and
    // per-device volume.
    var consecutiveAttemptsWithoutOpen = 0;
    var everOpened = false;
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
        everOpened = true;
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
      // Give up to polling only if WS has NEVER opened (permanent cert/capability
      // quirk). A socket that opened before and then dropped is a transient blip
      // — keep retrying WS so the TV stays a selectable output once it recovers.
      if (!everOpened && consecutiveAttemptsWithoutOpen >= GIVE_UP_AFTER) {
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
  // Read-only: a polling device doesn't register. Compat-mode audio
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

  // Stable per-install identity, mirroring the SPA's uiStore.clientId. Old
  // TVs lack crypto.randomUUID, so we mint a plain-random token and persist
  // it. Keyed separately from the SPA (these browsers never run the SPA, so
  // the two can't collide). The operator's "output by default" designation
  // in Settings → Devices sticks to this id across reloads, and it's what
  // makes the device addressable in active_output_device_ids / device_volumes.
  //
  // Key migration: this surface used to be called "TV mode" and persisted
  // under "tv-mode.*". Read the legacy key as a fallback and re-save it under
  // the new name, so an existing device keeps its identity (and therefore its
  // output designation + per-device volume) across the rename.
  function readPersisted(newKey, legacyKey) {
    try {
      if (!window.localStorage) return null;
      var stored = localStorage.getItem(newKey);
      if (stored) return stored;
      var legacy = localStorage.getItem(legacyKey);
      if (legacy) {
        try { localStorage.setItem(newKey, legacy); } catch (e) {}
        return legacy;
      }
    } catch (e) {}
    return null;
  }

  function getClientId() {
    var stored = readPersisted("compat-mode.client_id", "tv-mode.client_id");
    if (stored) return stored;
    var id = "compat-" + makeShortId() + makeShortId() + nowMs().toString(36);
    try { window.localStorage && localStorage.setItem("compat-mode.client_id", id); }
    catch (e) {}
    return id;
  }

  // Device name resolution:
  //   1. ?name=... URL param (one-time setup — also persists to localStorage)
  //   2. localStorage from a previous visit (legacy "tv-mode.name" migrates)
  //   3. generated default "Old TV (xxxxx)" — minted once, then persisted so
  //      the label (and the operator's device-list entry) is stable on reload.
  //      ("Old TV" names the physical device, not this mode — it also drives
  //      the TV glance icon in the device list.)
  // Bookmark `https://music.example/?compat&name=Living%20Room` once to claim
  // a friendlier label that survives subsequent reloads.
  function getDeviceName() {
    var fromUrl = getQueryParam("name");
    if (fromUrl) {
      try { window.localStorage && localStorage.setItem("compat-mode.name", fromUrl); }
      catch (e) {}
      return fromUrl;
    }
    var stored = readPersisted("compat-mode.name", "tv-mode.name");
    if (stored) return stored;
    var generated = "Old TV (" + makeShortId() + ")";
    try { window.localStorage && localStorage.setItem("compat-mode.name", generated); }
    catch (e) {}
    return generated;
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
    try { console.log("[compat-mode] crossfade supported:", crossfadeOk); } catch (e) {}
    var ui = renderPlayer();
    var engine = makeAudioEngine(crossfadeOk);
    engine.prime();

    var lastTrackId = null;
    var lastIsPlaying = false;
    // Whether the previously applied state had an interrupt active — drives
    // the "hard cut on lane switch" rule in applyState.
    var wasInterrupt = false;
    var clientId = getClientId();
    var deviceLabel = getDeviceName();
    ui.setDeviceName(deviceLabel);
    // This device's identity in PlayerState. In WebSocket mode it's our own
    // stable client_id (set in onOpen, mirroring the SPA) so the TV is a
    // first-class, operator-selectable output — the operator ticks it on in
    // the Speakers popover and can trim its per-device volume / save it
    // default-on. In polling mode it stays null — a poller can't register, so
    // it can never appear in active_output_device_ids; null makes
    // isThisDeviceActive() return true so it still works as a passive
    // always-on speaker (the only useful behaviour when it can't be selected).
    // your_device_id from the snapshot is empty by design and ignored.
    var myDeviceId = null;
    // In WS mode, the send function (captured in onOpen) — used for the 1 Hz
    // position reports below. null in polling mode (read-only transport).
    var wsSend = null;
    // Timeline state. The server owns the playback clock now, so position_ms
    // is current in every push; we interpolate between pushes locally so the
    // progress bar moves smoothly. currentTrackLengthMs comes from
    // /api/library/tracks/<id> via fetchTrack on each track change.
    var currentTrackLengthMs = 0;
    var serverPositionMs = 0;
    var serverPositionTimestamp = nowMs();
    // position_epoch of the last applied state. The server bumps it ONLY on
    // deliberate moves (seek / skip / loop restart / interrupt transitions);
    // we seek iff it changed. null = no state applied yet.
    var lastEpoch = null;

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
      // Lane pick: an interrupt takes over while present (alerts/stingers),
      // ambient otherwise. duck_to layering (music quietly under the
      // interrupt) needs two simultaneous streams — out of scope for this
      // fallback, the interrupt simply replaces the music and ambient
      // resumes at its server-preserved position afterwards.
      var itr = state.interrupt || null;
      var amb = state.ambient || {};
      var trackId = itr
        ? itr.current_track_id
        : ((amb.current_track_id == null) ? null : amb.current_track_id);
      var posMs = (itr ? itr.position_ms : amb.position_ms) || 0;
      var epoch = (typeof state.position_epoch === "number") ? state.position_epoch : 0;
      var trackChanged = trackId !== lastTrackId;
      // Gate playback on this device being a selected output (WS mode, where
      // we have an identity). In polling mode myDeviceId is null and
      // isThisDeviceActive() returns true — a passive always-on speaker.
      // An interrupt plays regardless of the pause flag (matching the SPA).
      var isPlaying = (itr ? true : !!state.is_playing) && isThisDeviceActive(state);
      var master = (typeof state.volume === "number") ? state.volume : 1;
      // Per-device trim (master × this device's device_volumes entry), so the
      // operator can tame a too-loud TV from the Speakers popover without
      // touching master — same fold the SPA engine does. Only applies in WS
      // mode (a poller has no addressable identity).
      var trim = 1;
      if (myDeviceId !== null && state.device_volumes &&
          typeof state.device_volumes[myDeviceId] === "number") {
        trim = state.device_volumes[myDeviceId];
      }
      // Crossfade only applies to ambient→ambient track changes; interrupt
      // transitions are hard cuts (an alert shouldn't fade in late).
      var laneSwitched = (itr !== null) !== wasInterrupt;
      var crossfadeMs = laneSwitched ? 0 : (state.crossfade_ms || 0);

      engine.setVolume(clamp01(master * trim));
      // Capture latest server position for the progress interpolator
      // (runs on its own 250 ms timer below).
      serverPositionMs = posMs;
      serverPositionTimestamp = nowMs();

      if (trackChanged) {
        if (trackId == null) {
          engine.clear();
          ui.setTitle("");
          ui.setArtist("");
          ui.setAlbum("");
          ui.setAlbumArt(null);
          currentTrackLengthMs = 0;
        } else {
          // posMs is 0 on a natural advance / fresh fire, and the real
          // position on a mid-track join (page load, lane return, reconnect)
          // — the pending-seek machinery lands it once metadata is in.
          engine.swap(trackId, crossfadeMs, isPlaying, posMs);
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
      } else if (trackId != null && lastEpoch !== null && epoch !== lastEpoch) {
        // Same track, epoch bumped: a deliberate server-side move (scrub-bar
        // seek, loop restart, cue start) — snap to it. Epoch unchanged means
        // NO seek, whatever the numbers say: volume changes, queue edits and
        // device joins no longer touch the element (this used to restart the
        // song on every such push, and re-seek every 2 s in polling mode).
        engine.seek(posMs);
        // A loop:track restart broadcasts position_ms 0 with is_playing
        // still true; if the element had ended (parked/paused) the seek
        // alone won't restart it, so nudge playback when we should be on.
        if (isPlaying) engine.setPlaying(true);
      }

      if (isPlaying !== lastIsPlaying) {
        engine.setPlaying(isPlaying);
        lastIsPlaying = isPlaying;
      }
      ui.setPlaying(isPlaying);
      wasInterrupt = itr !== null;
      lastEpoch = epoch;
    }

    // Position reports (WS mode only): 1 Hz drift corrections for the
    // server's playback clock, from the device that actually makes sound.
    // The server accepts them from guests as long as the operator has this
    // device toggled on as an output — self-gate on the same condition
    // (lastIsPlaying already folds it in) so we never send a rejectable one.
    setInterval(function () {
      if (!wsSend || myDeviceId === null || !lastIsPlaying) return;
      var pos = engine.getPositionMs();
      if (pos == null) return;
      wsSend({ type: "position_report", position_ms: pos });
    }, 1000);

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
      // Drop our WS identity — polling can't keep a registered device alive,
      // so the server has already pruned it. Clearing myDeviceId makes
      // isThisDeviceActive() fall back to "always play" (passive speaker
      // mode), the only useful behaviour when we can't be selected as an
      // output. No wsSend either — polling is read-only, so no position
      // reports (the server clock ticks on its own regardless).
      myDeviceId = null;
      wsSend = null;
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

    // No WebSocket at all (a very old TV): the polling path needs only XHR, so
    // use it directly instead of dead-ending. renderUnsupported is reserved for
    // browsers that can't even do XHR (see activateCompatMode).
    if (typeof WebSocket === "undefined") {
      startPolling();
      return;
    }

    makeWsClient(wsUrl, {
      onStatus: ui.setStatus,
      onOpen: function (send) {
        // Self-assign identity from our own stable client_id: the server's
        // your_device_id is empty by design (it doesn't know us until this
        // register arrives). This is what makes isThisDeviceActive() match
        // against active_output_device_ids and lets the operator pick this TV
        // in the Speakers popover, trim its volume, and save it default-on.
        myDeviceId = clientId;
        wsSend = send;
        send({ type: "register", name: deviceLabel, client_id: clientId });
      },
      onMessage: function (msg) {
        if (!msg || !msg.type) return;
        // your_device_id is empty by design now — identity is our own
        // client_id (set in onOpen). Snapshot and delta both just carry state.
        if (msg.type === "state_snapshot" || msg.type === "state_changed") {
          applyState(msg.state);
        }
      },
      onGiveUp: startPolling
    });
  }

  // ---- Bootstrap --------------------------------------------------------

  function hasCompatParam() {
    // ?compat (valueless) -> getQueryParam returns "" (present); absent ->
    // null. ?tv is the legacy alias, kept so old bookmarks keep working.
    return getQueryParam("compat") !== null || getQueryParam("tv") !== null;
  }

  // Whether the SPA owns #root and compat mode must stand down. True once the
  // bundle has booted (or React has already painted) — EXCEPT under an explicit
  // ?compat / ?tv preview, where main.tsx injected us on purpose and compat
  // mode wins. This is what stops a watchdog-injected copy from racing a bundle
  // that booted a beat after the watchdog fired.
  function spaOwnsRoot() {
    if (hasCompatParam()) return false;
    return reactMounted() || !!window.__SPA_BOOTED__;
  }

  function activateCompatMode() {
    // The only hard floor is XHR: a WebSocket-less browser still works over the
    // polling fallback (handled inside start()), so it's not dead-ended here.
    if (typeof XMLHttpRequest === "undefined") {
      renderUnsupported("This browser is too old to fetch playback state.");
      return;
    }
    renderStartScreen(function () { start(); });
  }

  // This script runs in three situations, all of which mean the SPA is NOT
  // rendering in this document:
  //   1. <script nomodule> on a browser too old for ES modules (index.html) —
  //      the fast capability path; a module-capable browser never fetches this.
  //   2. The index.html boot watchdog injecting it because the module bundle
  //      errored or never booted (a module-capable-but-too-old TV that can't
  //      parse the bundle's modern syntax) — the case `nomodule` alone misses.
  //   3. main.tsx injecting it for the ?compat / ?tv preview (React is skipped
  //      there).
  // __COMPAT_MODE_ACTIVE__ makes activation idempotent (paths 1 and 2 can both
  // fire on the same old TV around DOMready). spaOwnsRoot() is the safety net:
  // the old "wait Nms for React, then claim #root" paint timer that kidnapped a
  // crashed/slow SPA is gone — we only ever take over when the bundle has
  // demonstrably failed to run.
  whenReady(function () {
    if (window.__COMPAT_MODE_ACTIVE__ || spaOwnsRoot()) return;
    window.__COMPAT_MODE_ACTIVE__ = true;
    activateCompatMode();
  });
})();
