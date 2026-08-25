import { lazy, Suspense } from "react";

const LibraryTagEditor = lazy(async () => {
  const module = await import("./LibraryTagEditor");
  return { default: module.LibraryTagEditor };
});

export function LibraryTagsView() {
  return (
    <div className="assistant-analysis-view assistant-tags-view">
      <Suspense
        fallback={
          <section className="surface-card assistant-tag-workspace">
            <p className="muted">Loading mood tags…</p>
          </section>
        }
      >
        <LibraryTagEditor />
      </Suspense>
    </div>
  );
}
