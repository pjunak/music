/** Vitest global setup — extends `expect` with the RTL matchers
 *  (toBeInTheDocument, toHaveTextContent, etc.) so individual tests
 *  don't need the import. Loaded via `setupFiles` in vite.config.ts. */
import "@testing-library/jest-dom/vitest";

import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// Web Storage polyfill for the test env.
//
// Node 22+ ships an experimental built-in `localStorage`/`sessionStorage`
// global that is `undefined` unless the process is started with
// `--localstorage-file`. Under vitest's jsdom environment that experimental
// global shadows jsdom's own Storage (on Node 26 here both `globalThis` and
// `window` resolve to it, returning `undefined`). The first write to any
// zustand `persist` store — e.g. `useUiStore` — then throws
// "Cannot read properties of undefined (reading 'setItem')".
//
// Install a real in-memory Storage before the store modules load. setupFiles
// run before the test files that import those modules, and the experimental
// global is a configurable accessor, so a plain redefine takes precedence.
function makeStorage(): Storage {
  const data = new Map<string, string>();
  const storage = {
    get length(): number {
      return data.size;
    },
    clear(): void {
      data.clear();
    },
    getItem(key: string): string | null {
      return data.has(key) ? (data.get(key) as string) : null;
    },
    key(index: number): string | null {
      return Array.from(data.keys())[index] ?? null;
    },
    removeItem(key: string): void {
      data.delete(key);
    },
    setItem(key: string, value: string): void {
      data.set(key, String(value));
    },
  };
  return storage as Storage;
}

for (const name of ["localStorage", "sessionStorage"] as const) {
  const value = makeStorage();
  for (const target of [globalThis, (globalThis as { window?: unknown }).window]) {
    if (target) {
      Object.defineProperty(target, name, { value, configurable: true, writable: true });
    }
  }
}

// Tear the rendered tree down between tests so a leftover component
// from a prior test can't spy on later expects.
afterEach(() => {
  cleanup();
});

// jsdom doesn't implement scrollIntoView; FolderTree calls it for keyboard
// focus + auto-reveal, so give it a no-op rather than a crash.
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}
