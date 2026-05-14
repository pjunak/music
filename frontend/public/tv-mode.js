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
    r.style.cssText = PAGE_STYLE;
    var wrap = el("div", "padding:60px 40px;");
    var status = el("div", "font-size:18px;color:#888;margin-bottom:32px;height:24px;", "connecting...");
    var title = el("h1", "font-size:56px;margin:0 0 16px 0;word-wrap:break-word;line-height:1.2;", "-");
    var artist = el("h2", "font-size:32px;margin:0 0 32px 0;color:#bbb;font-weight:normal;", "");
    var playing = el("div", "font-size:24px;color:#4caf50;height:32px;", "paused");
    var footer = el("div",
      "position:fixed;bottom:20px;left:0;right:0;font-size:14px;color:#444;",
      "TV speaker mode — controlled from another device");
    wrap.appendChild(status);
    wrap.appendChild(title);
    wrap.appendChild(artist);
    wrap.appendChild(playing);
    r.appendChild(wrap);
    r.appendChild(footer);
    return {
      setStatus: function (text, color) {
        setText(status, text);
        status.style.color = color || "#888";
      },
      setTitle: function (text) { setText(title, text || "-"); },
      setArtist: function (text) { setText(artist, text || ""); },
      setPlaying: function (isPlaying) {
        setText(playing, isPlaying ? "▶ playing" : "⏸ paused");
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

    function connect() {
      if (stopped) return;
      handlers.onStatus("connecting...", "#888");
      try {
        ws = new WebSocket(url);
      } catch (e) {
        handlers.onStatus("connection failed", "#ff7373");
        scheduleReconnect();
        return;
      }
      ws.onopen = function () {
        backoff = 1000;
        handlers.onStatus("connected", "#4caf50");
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
        if (!stopped) {
          handlers.onStatus("disconnected — retrying", "#ff7373");
          scheduleReconnect();
        }
      };
    }

    function scheduleReconnect() {
      if (stopped) return;
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

  // ---- Main orchestration -----------------------------------------------

  function makeShortId() {
    var alphabet = "abcdefghjkmnpqrstuvwxyz23456789";
    var out = "";
    for (var i = 0; i < 5; i++) {
      out += alphabet.charAt(Math.floor(Math.random() * alphabet.length));
    }
    return out;
  }

  function start() {
    var crossfadeOk = supportsFractionalVolume();
    try { console.log("[tv-mode] crossfade supported:", crossfadeOk); } catch (e) {}
    var ui = renderPlayer();
    var engine = makeAudioEngine(crossfadeOk);
    engine.prime();

    var lastTrackId = null;
    var lastIsPlaying = false;
    var deviceLabel = "Old TV (" + makeShortId() + ")";

    function applyState(state) {
      if (!state) return;
      var amb = state.ambient || {};
      var trackId = (amb.current_track_id == null) ? null : amb.current_track_id;
      var posMs = amb.position_ms || 0;
      var isPlaying = !!state.is_playing;
      var vol = (typeof state.volume === "number") ? state.volume : 1;
      var crossfadeMs = state.crossfade_ms || 0;

      engine.setVolume(vol);

      if (trackId !== lastTrackId) {
        if (trackId == null) {
          engine.clear();
          ui.setTitle("-");
          ui.setArtist("");
        } else {
          engine.swap(trackId, crossfadeMs, isPlaying);
          ui.setTitle("Track " + trackId);
          ui.setArtist("");
          fetchTrack(trackId, function (err, t) {
            if (err || !t) return;
            ui.setTitle(t.display_title || t.title || ("Track " + trackId));
            var line = t.artist || "";
            if (t.album) line = line ? (line + " — " + t.album) : t.album;
            ui.setArtist(line);
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

    var wsScheme = (window.location.protocol === "https:") ? "wss://" : "ws://";
    var wsUrl = wsScheme + window.location.host + "/api/ws";
    makeWsClient(wsUrl, {
      onStatus: ui.setStatus,
      onOpen: function (send) {
        send({ type: "register", name: deviceLabel, capabilities: [] });
      },
      onMessage: function (msg) {
        if (!msg || !msg.type) return;
        if (msg.type === "state_snapshot" || msg.type === "state_changed") {
          applyState(msg.state);
        }
      }
    });
  }

  // ---- Bootstrap --------------------------------------------------------

  whenReady(function () {
    var delay = forced() ? 0 : 500;
    setTimeout(function () {
      if (!forced() && reactMounted()) return;
      if (typeof WebSocket === "undefined") {
        renderUnsupported("WebSocket support is required.");
        return;
      }
      renderStartScreen(function () { start(); });
    }, delay);
  });
})();
