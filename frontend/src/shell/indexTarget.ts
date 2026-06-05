import type { AuthStatus } from "@/core/auth";

export type IndexTarget = "spinner" | "console" | "tv";

/** What the bare `/` index should render, by auth status. Pure so the routing
 *  decision is unit-testable without mounting the shell:
 *  - unknown       → spinner (don't flash TV at someone who's actually the
 *                    operator, don't redirect yet)
 *  - authenticated → bounce to the /console workspace
 *  - anonymous     → the guest TV view
 *
 *  Split into its own (non-component) module so `routeGuards.tsx` exports only
 *  components — keeps React Fast Refresh happy (same reason confirmDialog.ts is
 *  split from ConfirmDialogHost.tsx). */
export function indexTarget(status: AuthStatus): IndexTarget {
  if (status === "unknown") return "spinner";
  return status === "authenticated" ? "console" : "tv";
}
