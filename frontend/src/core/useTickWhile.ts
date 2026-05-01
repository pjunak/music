import { useEffect, useState } from "react";

/** Force the calling component to re-render at a fixed interval while
 *  `enabled` is true. Used to make dead-reckoned values (like ambient
 *  playback position computed from `stateReceivedAt`) advance visually
 *  without needing an extra store update on every frame.
 *
 *  When `enabled` is false the timer is torn down — this is the whole
 *  point vs `setInterval` directly. The returned tick value is always
 *  monotonic; consumers don't need to read it, but it's available for
 *  use as a dependency or progress indicator. */
export function useTickWhile(enabled: boolean, intervalMs: number): number {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    if (!enabled) return;
    const id = window.setInterval(() => {
      setTick((t) => t + 1);
    }, intervalMs);
    return () => window.clearInterval(id);
  }, [enabled, intervalMs]);
  return tick;
}
