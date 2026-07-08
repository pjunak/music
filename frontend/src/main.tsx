import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";

import { ErrorBoundary } from "@/components/ErrorBoundary";
import App from "@/App";
import "@/styles/global.css";

declare global {
  interface Window {
    __SPA_BOOTED__?: boolean;
  }
}

// Boot beacon. Reaching this line means the whole module graph parsed,
// instantiated, and execution arrived at the entrypoint — i.e. this browser can
// actually run the bundle. The classic-script watchdog in index.html waits for
// this flag; if it never arrives (an old TV browser that supports <script
// type=module> but chokes on the bundle's modern syntax, e.g. `??`, so the
// module fails to parse and #root is left empty), the watchdog loads the
// compatibility fallback instead of leaving a blank screen. It keys on *bundle
// execution*, not React paint, so — unlike the old 500 ms paint timer this
// replaces — it can't false-fire on a capable-but-slow browser. Set
// unconditionally (before the ?compat branch too) so the signal is purely
// "the bundle ran".
window.__SPA_BOOTED__ = true;

const rootElement = document.getElementById("root");
if (!rootElement) throw new Error("root element not found");

// ?compat (legacy alias: ?tv) previews the compatibility fallback
// (compat-mode.js) on a module-capable browser. compat-mode.js ships as
// `<script nomodule>`, so a modern browser never auto-loads it (the race-free
// capability split — see index.html). For an explicit preview we inject it on
// demand and skip React, so the same URL drives both surfaces.
const params = new URLSearchParams(window.location.search);
if (params.has("compat") || params.has("tv")) {
  const s = document.createElement("script");
  s.src = "/compat-mode.js";
  document.body.appendChild(s);
} else {
  ReactDOM.createRoot(rootElement).render(
    <React.StrictMode>
      <BrowserRouter>
        <ErrorBoundary>
          <App />
        </ErrorBoundary>
      </BrowserRouter>
    </React.StrictMode>,
  );
}
