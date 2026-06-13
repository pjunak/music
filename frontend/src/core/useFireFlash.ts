import { useCallback, useEffect, useRef, useState } from "react";

/** Brief "fired!" highlight for trigger tiles (cues, SFX). Firing is otherwise
 *  invisible on the tile itself, so a ~260ms accent flash confirms the press
 *  landed — important for a live performance surface. Returns the key
 *  currently flashing and a `flash(key)` trigger. */
export function useFireFlash(): [string | null, (key: string) => void] {
  const [fired, setFired] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const flash = useCallback((key: string) => {
    if (timer.current) clearTimeout(timer.current);
    setFired(key);
    timer.current = setTimeout(() => setFired(null), 260);
  }, []);

  return [fired, flash];
}
