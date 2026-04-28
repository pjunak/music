import { create } from "zustand";

export type ToastKind = "info" | "success" | "warn" | "error";

export interface Toast {
  id: number;
  kind: ToastKind;
  message: string;
  /** Optional second line (e.g. error detail). */
  detail?: string;
  /** ms until auto-dismiss. 0 = sticky. */
  ttl: number;
}

interface ToastStore {
  toasts: Toast[];
  push: (kind: ToastKind, message: string, opts?: { detail?: string; ttl?: number }) => number;
  dismiss: (id: number) => void;
}

let nextId = 1;

export const useToastStore = create<ToastStore>()((set, get) => ({
  toasts: [],
  push: (kind, message, opts = {}) => {
    const id = nextId++;
    const ttl = opts.ttl ?? (kind === "error" ? 7000 : 3500);
    const detail = opts.detail;
    set((s) => ({
      toasts: [
        ...s.toasts,
        detail !== undefined ? { id, kind, message, detail, ttl } : { id, kind, message, ttl },
      ],
    }));
    if (ttl > 0) {
      window.setTimeout(() => get().dismiss(id), ttl);
    }
    return id;
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

/** Convenience facade — same shape as the store but module-callable so
 *  non-React code (api error handlers) can fire toasts without hooks. */
export const toast = {
  info: (m: string, detail?: string) =>
    useToastStore.getState().push("info", m, detail !== undefined ? { detail } : undefined),
  success: (m: string, detail?: string) =>
    useToastStore.getState().push("success", m, detail !== undefined ? { detail } : undefined),
  warn: (m: string, detail?: string) =>
    useToastStore.getState().push("warn", m, detail !== undefined ? { detail } : undefined),
  error: (m: string, detail?: string) =>
    useToastStore.getState().push("error", m, detail !== undefined ? { detail } : undefined),
};
