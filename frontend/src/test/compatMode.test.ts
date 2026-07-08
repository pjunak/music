/** Tests for the activation guard in the real public/compat-mode.js.
 *
 *  We execute the actual shipped IIFE against the jsdom globals (its
 *  whenReady() runs synchronously because readyState is "complete" in the test
 *  env), then observe whether it took over `#root`. Compat mode must take over
 *  only when the SPA bundle genuinely isn't running — and must never clobber a
 *  booted or already-painted SPA, the regression that the old paint-timer
 *  caused. The `?compat` preview (legacy alias `?tv`) is the one deliberate
 *  exception. */
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

declare global {
  interface Window {
    __COMPAT_MODE_ACTIVE__?: boolean;
  }
}

/** Resolves from the vitest cwd (the frontend dir under `npm run test`) with a
 *  fallback for a repo-root cwd — import.meta.url isn't a file: URL once vitest
 *  transforms the module. */
function readProjectFile(relPath: string): string {
  for (const root of [process.cwd(), resolve(process.cwd(), "frontend")]) {
    const p = resolve(root, relPath);
    if (existsSync(p)) return readFileSync(p, "utf8");
  }
  throw new Error(`could not locate ${relPath} from cwd ${process.cwd()}`);
}

const COMPAT_MODE_SOURCE = readProjectFile("public/compat-mode.js");

/** Run the real compat-mode.js IIFE against the jsdom globals. A trailing
 *  DOMContentLoaded is dispatched in case the env reported readyState
 *  "loading" (then whenReady deferred to it); when readyState is "complete"
 *  the activation already ran synchronously and the dispatch is a harmless
 *  no-op (compat-mode added no listener for it). */
function runCompatMode(): void {
  new Function(COMPAT_MODE_SOURCE)();
  document.dispatchEvent(new Event("DOMContentLoaded"));
}

function setRoot(content: "empty" | "spa"): HTMLElement {
  document.body.innerHTML = "";
  const root = document.createElement("div");
  root.id = "root";
  if (content === "spa") {
    const child = document.createElement("div");
    child.textContent = "SPA shell";
    root.appendChild(child);
  }
  document.body.appendChild(root);
  return root;
}

function setUrl(search: string): void {
  window.history.pushState(null, "", "/" + search);
}

beforeEach(() => {
  delete window.__SPA_BOOTED__;
  delete window.__COMPAT_MODE_ACTIVE__;
  setUrl("");
  setRoot("empty");
  window.localStorage.clear();
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.body.innerHTML = "";
});

describe("compat-mode.js activation guard", () => {
  it("activates when the bundle failed (empty #root, no boot flag, no ?compat)", () => {
    const root = setRoot("empty");
    runCompatMode();
    expect(root.textContent).toContain("Compatibility mode");
    expect(window.__COMPAT_MODE_ACTIVE__).toBe(true);
  });

  it("stands down when the bundle booted — even though #root is still empty (the race)", () => {
    const root = setRoot("empty");
    window.__SPA_BOOTED__ = true; // beacon set before React's first paint
    runCompatMode();
    expect(root.textContent).not.toContain("Compatibility mode");
    expect(root.children).toHaveLength(0);
    expect(window.__COMPAT_MODE_ACTIVE__).toBeUndefined();
  });

  it("stands down when React has already mounted into #root", () => {
    const root = setRoot("spa");
    runCompatMode();
    expect(root.textContent).toContain("SPA shell");
    expect(root.textContent).not.toContain("Compatibility mode");
  });

  it("activates under an explicit ?compat preview even when the SPA booted", () => {
    const root = setRoot("empty");
    window.__SPA_BOOTED__ = true;
    setUrl("?compat");
    runCompatMode();
    expect(root.textContent).toContain("Compatibility mode");
  });

  it("still honors the legacy ?tv preview alias (old bookmarks)", () => {
    const root = setRoot("empty");
    window.__SPA_BOOTED__ = true;
    setUrl("?tv");
    runCompatMode();
    expect(root.textContent).toContain("Compatibility mode");
  });

  it("is idempotent: bails when compat mode is already active", () => {
    const root = setRoot("empty");
    window.__COMPAT_MODE_ACTIVE__ = true;
    runCompatMode();
    expect(root.textContent).not.toContain("Compatibility mode");
    expect(root.children).toHaveLength(0);
  });

  it("shows the 'browser too old' screen (not the player) when XHR is unavailable", () => {
    const root = setRoot("empty");
    vi.stubGlobal("XMLHttpRequest", undefined);
    runCompatMode();
    expect(root.textContent).toContain("Browser too old");
    expect(root.textContent).not.toContain("Compatibility mode");
  });
});

describe("compat-mode.js identity migration", () => {
  it("keeps a legacy tv-mode client_id and name, re-keyed under compat-mode.*", () => {
    // A device that ran this surface before the rename must keep its stable
    // identity — the operator's output designation and per-device volume are
    // keyed on it server-side.
    window.localStorage.setItem("tv-mode.client_id", "tv-legacy123");
    window.localStorage.setItem("tv-mode.name", "Living Room TV");
    const root = setRoot("empty");
    runCompatMode();
    // Press "Click / OK to start" so start() runs getClientId()/getDeviceName().
    const startButton = root.querySelector("button");
    expect(startButton).not.toBeNull();
    startButton?.click();
    expect(window.localStorage.getItem("compat-mode.client_id")).toBe("tv-legacy123");
    expect(window.localStorage.getItem("compat-mode.name")).toBe("Living Room TV");
  });
});
