import { useEffect, useRef } from "react";
import type { ReactNode } from "react";

import { useUiStore } from "@/core/uiStore";

const RAIL_MIN = 240;
const RAIL_MAX = 560;

export function LibrarySidebarRail({ children }: { children: ReactNode }) {
  const setWidth = useUiStore((state) => state.setLibraryTreeWidth);
  const storedWidth = useUiStore((state) => state.libraryTreeWidth);
  const asideRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    const host = asideRef.current?.closest<HTMLElement>(".library-view");
    if (host === undefined || host === null) return;
    if (storedWidth === null) host.style.removeProperty("--rail-tree");
    else host.style.setProperty("--rail-tree", `${storedWidth}px`);
  }, [storedWidth]);

  function onPointerDown(event: React.PointerEvent<HTMLDivElement>) {
    const aside = asideRef.current;
    const host = aside?.closest<HTMLElement>(".library-view");
    if (aside === null || aside === undefined || host === null || host === undefined) return;
    event.preventDefault();
    const handle = event.currentTarget;
    handle.setPointerCapture(event.pointerId);
    const startX = event.clientX;
    const startWidth = aside.getBoundingClientRect().width;
    let lastWidth = startWidth;
    const onMove = (moveEvent: PointerEvent) => {
      moveEvent.preventDefault();
      lastWidth = Math.round(
        Math.max(
          RAIL_MIN,
          Math.min(RAIL_MAX, startWidth + (moveEvent.clientX - startX)),
        ),
      );
      host.style.setProperty("--rail-tree", `${lastWidth}px`);
    };
    const onUp = () => {
      handle.removeEventListener("pointermove", onMove);
      handle.removeEventListener("pointerup", onUp);
      handle.removeEventListener("pointercancel", onUp);
      setWidth(lastWidth);
    };
    handle.addEventListener("pointermove", onMove);
    handle.addEventListener("pointerup", onUp);
    handle.addEventListener("pointercancel", onUp);
  }

  function onKeyDown(event: React.KeyboardEvent<HTMLDivElement>) {
    const aside = asideRef.current;
    if (aside === null) return;
    const current = Math.round(aside.getBoundingClientRect().width);
    const step = event.shiftKey ? 64 : 16;
    let next: number | null | undefined;
    switch (event.key) {
      case "ArrowLeft":
        next = Math.max(RAIL_MIN, current - step);
        break;
      case "ArrowRight":
        next = Math.min(RAIL_MAX, current + step);
        break;
      case "Home":
        next = RAIL_MIN;
        break;
      case "End":
        next = RAIL_MAX;
        break;
      case "Delete":
      case "Backspace":
        next = null;
        break;
      default:
        return;
    }
    event.preventDefault();
    setWidth(next);
  }

  return (
    <aside ref={asideRef} className="library-sidebar">
      {children}
      <div
        className="rail-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize the folder tree column"
        aria-valuemin={RAIL_MIN}
        aria-valuemax={RAIL_MAX}
        aria-valuenow={storedWidth ?? undefined}
        tabIndex={0}
        title="Drag to resize — double-click or Delete to reset; arrow keys when focused"
        onPointerDown={onPointerDown}
        onDoubleClick={() => setWidth(null)}
        onKeyDown={onKeyDown}
      />
    </aside>
  );
}
