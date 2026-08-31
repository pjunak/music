import { lazy, Suspense } from "react";

const LibraryTagEditor = lazy(async () => {
  const module = await import("./LibraryTagEditor");
  return { default: module.LibraryTagEditor };
});

export function LibraryTagsView() {
  return (
    <Suspense
      fallback={
        <div className="library-view assistant-context-view assistant-tags-view">
          <p className="muted">Loading mood tags…</p>
        </div>
      }
    >
      <LibraryTagEditor />
    </Suspense>
  );
}
