/** Vitest global setup — extends `expect` with the RTL matchers
 *  (toBeInTheDocument, toHaveTextContent, etc.) so individual tests
 *  don't need the import. Loaded via `setupFiles` in vite.config.ts. */
import "@testing-library/jest-dom/vitest";

import { afterEach } from "vitest";
import { cleanup } from "@testing-library/react";

// Tear the rendered tree down between tests so a leftover component
// from a prior test can't spy on later expects.
afterEach(() => {
  cleanup();
});
