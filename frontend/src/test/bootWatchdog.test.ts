/** Tests for the generated ES5 SPA boot watchdog
 *  and its contract with the boot beacon in main.tsx.
 *
 *  We generate the same watchdog that Vite inlines into index.html, then run
 *  it with an injected window/document so each case is
 *  fully isolated (fresh closure + fresh listener registry + fresh timer set).
 *  The scenarios simulate the reported bug: an older TV that supports
 *  `<script type=module>` yet can't parse the bundle's modern syntax, so the
 *  module never boots and `#root` is left empty. The watchdog must load the
 *  compatibility fallback in exactly that case — and stand down whenever the
 *  bundle DID run. */
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { generateCompatAssets } from "../../scripts/compat-build.mts";

/** Read a repo file as text. Resolves from the vitest cwd (the frontend dir
 *  under `npm run test`) with a fallback for a repo-root cwd. (import.meta.url
 *  isn't a file: URL once vitest transforms the module, so fs + cwd it is.) */
function readProjectFile(relPath: string): string {
  for (const root of [process.cwd(), resolve(process.cwd(), "frontend")]) {
    const p = resolve(root, relPath);
    if (existsSync(p)) return readFileSync(p, "utf8");
  }
  throw new Error(`could not locate ${relPath} from cwd ${process.cwd()}`);
}

interface InjectedScript {
  tagName: string;
  src: string;
}
interface ErrorEventLike {
  message?: string;
  target?: { tagName?: string } | null;
}
interface WatchdogWindow {
  __SPA_BOOTED__?: boolean;
  __COMPAT_MODE_ACTIVE__?: boolean;
  addEventListener(
    type: string,
    fn: (e?: ErrorEventLike) => void,
    capture?: boolean,
  ): void;
}
interface WatchdogDocument {
  createElement(tag: string): InjectedScript;
  body: { appendChild(node: InjectedScript): void };
  documentElement: { appendChild(node: InjectedScript): void };
}
type RunWatchdog = (window: WatchdogWindow, document: WatchdogDocument) => void;

function findFrontendRoot(): string {
  for (const root of [process.cwd(), resolve(process.cwd(), "frontend")]) {
    try {
      generateCompatAssets(root);
      return root;
    } catch {
      // Try the repo-root fallback when Vitest was launched above frontend.
    }
  }
  throw new Error(`could not locate compatibility sources from cwd ${process.cwd()}`);
}

const WATCHDOG_SOURCE = generateCompatAssets(findFrontendRoot()).bootWatchdog;

interface Harness {
  win: WatchdogWindow;
  injected: InjectedScript[];
  fireError(e: ErrorEventLike): void;
  fireLoad(): void;
  run(): void;
}

function makeHarness(boot?: { spaBooted?: boolean; compatActive?: boolean }): Harness {
  const injected: InjectedScript[] = [];
  const listeners: Record<string, Array<(e?: ErrorEventLike) => void>> = {
    error: [],
    load: [],
  };
  const win: WatchdogWindow = {
    addEventListener(type, fn) {
      (listeners[type] ??= []).push(fn);
    },
  };
  if (boot?.spaBooted) win.__SPA_BOOTED__ = true;
  if (boot?.compatActive) win.__COMPAT_MODE_ACTIVE__ = true;
  const append = (node: InjectedScript): void => {
    injected.push(node);
  };
  const doc: WatchdogDocument = {
    createElement: (tag) => ({ tagName: tag.toUpperCase(), src: "" }),
    body: { appendChild: append },
    documentElement: { appendChild: append },
  };
  return {
    win,
    injected,
    fireError: (e) => listeners.error.forEach((fn) => fn(e)),
    fireLoad: () => listeners.load.forEach((fn) => fn()),
    run: () =>
      (new Function("window", "document", WATCHDOG_SOURCE) as unknown as RunWatchdog)(
        win,
        doc,
      ),
  };
}

describe("boot watchdog (index.html)", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  describe("activates the compatibility fallback when the bundle truly fails to run", () => {
    it("injects compat-mode.js on a pre-boot parse error (the old-TV `??` SyntaxError)", () => {
      const h = makeHarness();
      h.run();
      h.fireError({ message: "Uncaught SyntaxError: Unexpected token '?'" });
      vi.advanceTimersByTime(250);
      expect(h.injected.map((s) => s.src)).toEqual(["/compat-mode.js"]);
    });

    it("injects when the bundle fails to fetch (resource error on the <script>)", () => {
      const h = makeHarness();
      h.run();
      h.fireError({ target: { tagName: "SCRIPT" } });
      vi.advanceTimersByTime(250);
      expect(h.injected).toHaveLength(1);
    });

    it("injects on the load+grace backstop when nothing errors but boot never happens", () => {
      const h = makeHarness();
      h.run();
      h.fireLoad();
      vi.advanceTimersByTime(1500);
      expect(h.injected).toHaveLength(1);
    });

    it("injects on the absolute backstop if `load` never fires (stalled fetch)", () => {
      const h = makeHarness();
      h.run();
      vi.advanceTimersByTime(10000);
      expect(h.injected).toHaveLength(1);
    });
  });

  describe("stands down whenever the bundle is (or is becoming) live", () => {
    it("never injects once the bundle has booted — even if a stray error fires after", () => {
      const h = makeHarness({ spaBooted: true });
      h.run();
      h.fireError({ message: "late non-fatal error" });
      h.fireLoad();
      vi.advanceTimersByTime(10000);
      expect(h.injected).toHaveLength(0);
    });

    it("lets a near-simultaneous boot win the race (error, then boot within the 250ms grace)", () => {
      const h = makeHarness();
      h.run();
      h.fireError({ message: "transient pre-boot error" });
      h.win.__SPA_BOOTED__ = true; // boot lands a beat after the error
      vi.advanceTimersByTime(250);
      expect(h.injected).toHaveLength(0);
    });

    it("stays inert when compat mode is already active (the nomodule path already ran)", () => {
      const h = makeHarness({ compatActive: true });
      h.run();
      h.fireError({ message: "err" });
      h.fireLoad();
      vi.advanceTimersByTime(10000);
      expect(h.injected).toHaveLength(0);
    });
  });

  it("rescues at most once across many failure signals (no inject loop)", () => {
    const h = makeHarness();
    h.run();
    h.fireError({ message: "e1" });
    vi.advanceTimersByTime(250);
    h.fireError({ message: "e2" });
    h.fireLoad();
    vi.advanceTimersByTime(10000);
    expect(h.injected).toHaveLength(1);
  });
});

describe("boot beacon contract (main.tsx)", () => {
  it("sets window.__SPA_BOOTED__ before createRoot, so the watchdog can rely on it", () => {
    const src = readProjectFile("src/main.tsx");
    const beaconAt = src.indexOf("window.__SPA_BOOTED__ = true");
    const rootAt = src.indexOf("createRoot");
    expect(beaconAt).toBeGreaterThan(-1);
    expect(rootAt).toBeGreaterThan(-1);
    expect(beaconAt).toBeLessThan(rootAt);
  });

  it("reloads through the current index when a deployed route chunk is stale", () => {
    const src = readProjectFile("src/main.tsx");
    const handlerAt = src.indexOf('addEventListener("vite:preloadError"');
    const preventDefaultAt = src.indexOf("event.preventDefault()", handlerAt);
    const reloadAt = src.indexOf("window.location.reload()", handlerAt);
    expect(handlerAt).toBeGreaterThan(-1);
    expect(preventDefaultAt).toBeGreaterThan(handlerAt);
    expect(reloadAt).toBeGreaterThan(preventDefaultAt);
  });
});
