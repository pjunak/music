import React from "react";
import ReactDOM from "react-dom/client";
import { BrowserRouter } from "react-router-dom";

import App from "@/App";
import "@/styles/global.css";

const rootElement = document.getElementById("root");
if (!rootElement) throw new Error("root element not found");

// ?tv forces the legacy fallback player (tv-mode.js) to take over #root so
// the same URL can be used for in-browser testing of the TV mode.
if (!new URLSearchParams(window.location.search).has("tv")) {
  ReactDOM.createRoot(rootElement).render(
    <React.StrictMode>
      <BrowserRouter>
        <App />
      </BrowserRouter>
    </React.StrictMode>,
  );
}
