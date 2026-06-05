import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";

import { ErrorBoundary } from "@/components/ErrorBoundary";
import App from "@/App";
import "@/styles/global.css";

const rootElement = document.getElementById("root");
if (!rootElement) throw new Error("root element not found");

// ?tv previews the legacy TV fallback (tv-mode.js) on a module-capable browser.
// tv-mode.js ships as `<script nomodule>`, so a modern browser never auto-loads
// it (the race-free capability split — see index.html). For an explicit preview
// we inject it on demand and skip React, so the same URL drives both surfaces.
if (new URLSearchParams(window.location.search).has("tv")) {
  const s = document.createElement("script");
  s.src = "/tv-mode.js";
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
