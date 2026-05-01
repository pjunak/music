import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useDebouncedValue } from "./useDebouncedValue";

/** Verifies the hook's two contracts:
 *   1. The debounced value lags the input by exactly the configured ms.
 *   2. Rapid changes reset the timer (only the latest value lands). */

describe("useDebouncedValue", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns the initial value synchronously", () => {
    const { result } = renderHook(() => useDebouncedValue("first", 200));
    expect(result.current).toBe("first");
  });

  it("delays propagation until the debounce window elapses", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebouncedValue(value, 200),
      { initialProps: { value: "first" } },
    );

    rerender({ value: "second" });
    expect(result.current).toBe("first"); // still old, timer pending

    act(() => {
      vi.advanceTimersByTime(199);
    });
    expect(result.current).toBe("first"); // one ms shy

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current).toBe("second"); // exactly at threshold
  });

  it("collapses rapid changes into a single trailing emission", () => {
    const { result, rerender } = renderHook(
      ({ value }) => useDebouncedValue(value, 200),
      { initialProps: { value: "a" } },
    );
    rerender({ value: "b" });
    act(() => vi.advanceTimersByTime(100));
    rerender({ value: "c" });
    act(() => vi.advanceTimersByTime(100));
    rerender({ value: "d" });
    act(() => vi.advanceTimersByTime(199));
    expect(result.current).toBe("a"); // every keystroke reset the timer

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(result.current).toBe("d"); // only the latest value emerges
  });
});
