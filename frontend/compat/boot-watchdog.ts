/* Classic ES5 watchdog for browsers that recognize modules but cannot parse
   the SPA bundle. Keep this source dependency-free and syntax-compatible with
   ES5; scripts/compat-build.mts validates the generated output. */
(function () {
  var rescued = false;
  function rescueToCompatMode() {
    if (rescued || window.__SPA_BOOTED__ || window.__COMPAT_MODE_ACTIVE__) return;
    rescued = true;
    var s = document.createElement("script");
    s.src = "/compat-mode.js";
    (document.body || document.documentElement).appendChild(s);
  }
  // Fast path: a script parse/eval error (the `??` SyntaxError) or a failed
  // bundle fetch. The grace period lets a nearly simultaneous SPA boot win.
  window.addEventListener("error", function (e: ErrorEvent) {
    if (window.__SPA_BOOTED__) return;
    var t = e && e.target as HTMLScriptElement | null;
    if ((t && t.tagName === "SCRIPT") || (e && e.message)) {
      setTimeout(rescueToCompatMode, 250);
    }
  }, true);
  // The load backstop covers silent parse failures that do not raise an event.
  window.addEventListener("load", function () {
    setTimeout(rescueToCompatMode, 1500);
  });
  // Cover a stalled module fetch for which window.load never fires.
  setTimeout(rescueToCompatMode, 10000);
})();
