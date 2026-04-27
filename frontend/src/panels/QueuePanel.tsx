import { usePlayerStore } from "@/core/playerStore";

export function QueuePanel() {
  const queue = usePlayerStore((s) => s.state?.ambient.queue ?? []);
  const current = usePlayerStore((s) => s.state?.ambient.current_beets_id ?? null);

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
