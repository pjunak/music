import { create } from "zustand";

/** Promise-based confirm-dialog API. Sits in a `.ts` file (not `.tsx`)
 *  on purpose: Vite's Fast Refresh requires component files to export
 *  only components, so the store + helper function had to come out of
 *  ConfirmDialog.tsx to silence the warning. */

interface ConfirmRequest {
  title: string;
  body?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  /** "danger" for destructive actions; styles the confirm button red. */
  tone?: "danger" | "primary";
  resolve: (ok: boolean) => void;
}

interface ConfirmStore {
  current: ConfirmRequest | null;
  open: (req: Omit<ConfirmRequest, "resolve">) => Promise<boolean>;
  resolve: (ok: boolean) => void;
}

export const useConfirmStore = create<ConfirmStore>()((set, get) => ({
  current: null,
  open: (req) =>
    new Promise<boolean>((resolve) => {
      // If a confirm is already open, queue behaviour: reject the previous one.
      const prev = get().current;
      if (prev) prev.resolve(false);
      set({ current: { ...req, resolve } });
    }),
  resolve: (ok) => {
    const cur = get().current;
    if (cur) {
      cur.resolve(ok);
      set({ current: null });
    }
  },
}));

/** Promise-based confirm that pops a styled modal. Use this everywhere
 *  instead of `window.confirm` so the look matches the app and we can
 *  trap focus / keyboard correctly. */
export function confirmDialog(req: {
  title: string;
  body?: string;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: "danger" | "primary";
}): Promise<boolean> {
  return useConfirmStore.getState().open(req);
}
