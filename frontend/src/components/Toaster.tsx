import type { ReactNode } from "react";

import { useToastStore } from "@/core/toast";
import type { ToastKind } from "@/core/toast";

import { CheckIcon, ErrorIcon, InfoIcon, WarnIcon, XIcon } from "./icons";

const ICONS: Record<ToastKind, ReactNode> = {
  info: <InfoIcon />,
  success: <CheckIcon />,
  warn: <WarnIcon />,
  error: <ErrorIcon />,
};

export function Toaster() {
  const toasts = useToastStore((s) => s.toasts);
  const dismiss = useToastStore((s) => s.dismiss);

  // Always-mounted live region: present before any toast so the screen
  // reader has it under observation and announces additions as they arrive.
  return (
    <div
      className="toaster"
      role="region"
      aria-label="Notifications"
      aria-live="polite"
      aria-atomic="false"
    >
      {toasts.map((t) => (
        <div
          key={t.id}
          className={`toast toast-${t.kind}`}
          role={t.kind === "error" ? "alert" : "status"}
        >
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
            <XIcon />
          </button>
        </div>
      ))}
    </div>
  );
}
