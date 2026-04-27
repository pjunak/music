import { usePlayerStore } from "@/core/playerStore";

export function QueuePanel() {
  // Select `state` once and apply fallbacks in render. Returning a fresh
  // `[]` from inside a zustand selector creates a new reference on every
  // render and fails Object.is comparison, causing an infinite re-render
  // loop until the first WS state snapshot arrives.
  const state = usePlayerStore((s) => s.state);
  const queue = state?.ambient.queue ?? [];
  const current = state?.ambient.current_beets_id ?? null;

  return (
    <section className="panel queue-panel">
      <h2>Queue</h2>
      {current === null && queue.length === 0 ? (
        <p className="muted small">Queue empty.</p>
      ) : (
        <ol className="queue-list">
          {current !== null ? (
            <li className="queue-current">▶ #{current}</li>
          ) : null}
          {queue.map((bid, i) => (
            <li key={`${bid}-${i}`}>#{bid}</li>
          ))}
        </ol>
      )}
    </section>
  );
}
