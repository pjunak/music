import { useEffect, useState } from "react";

/** Returns a debounced copy of `value`. The returned value lags by `ms`
 *  after the input stops changing, so consumers (search effects, expensive
 *  fetches) can react only when typing has paused.
 *
 *  Generic over T so it works for strings, numbers, even structured input
 *  state — just remember that two `useDebouncedValue` calls with the same
 *  underlying value but different references will fire independently. */
export function useDebouncedValue<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const t = window.setTimeout(() => setDebounced(value), ms);
    return () => window.clearTimeout(t);
  }, [value, ms]);
  return debounced;
}
