import { useToastStore } from "@/core/toast";
import type { ToastKind } from "@/core/toast";

const ICONS: Record<ToastKind, string> = {
  info: "ℹ",
  success: "✓",
  warn: "⚠",
  error: "✕",
};

export function Toaster() {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);

  if (toasts.length === 0) return null;

  return (
    <div className="toaster" role="region" aria-label="Notifications">
      {toasts.map((t) => (
        <div key={t.id} className={`toast toast-${t.kind}`} role="status">
          <span className="toast-icon" aria-hidden="true">
            {ICONS[t.kind]}
          </span>
          <div className="toast-body">
            <div className="toast-message">{t.message}</div>
            {t.detail !== undefined ? (
              <div className="toast-detail">{t.detail}</div>
            ) : null}
          </div>
          <button
            type="button"
            className="toast-close btn-ghost"
            onClick={() => dismiss(t.id)}
            aria-label="Dismiss"
          >
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
