import { create } from "zustand";

import type { UploadConflict } from "@/core/api";

/** Promise-based "files already exist" chooser, same shape as confirmDialog
 *  but with three outcomes (overwrite / skip / keep both) instead of yes/no.
 *  Lives in a `.ts` file so Fast Refresh keeps the host component file
 *  component-only (see confirmDialog.ts for the same reasoning). */

interface ConflictRequest {
  /** How many of the about-to-upload files collide with existing ones. */
  count: number;
  /** Total files in the drop (for "N of M" phrasing). */
  total: number;
  /** A few colliding names to show; the rest are summarised as "+N more". */
  sampleNames: string[];
  resolve: (choice: UploadConflict | null) => void;
}

interface ConflictStore {
  current: ConflictRequest | null;
  open: (req: Omit<ConflictRequest, "resolve">) => Promise<UploadConflict | null>;
  resolve: (choice: UploadConflict | null) => void;
}

export const useConflictStore = create<ConflictStore>()((set, get) => ({
  current: null,
  open: (req) =>
    new Promise<UploadConflict | null>((resolve) => {
      // Only one chooser at a time; cancel any previous (resolves to null).
      const prev = get().current;
      if (prev) prev.resolve(null);
      set({ current: { ...req, resolve } });
    }),
  resolve: (choice) => {
    const cur = get().current;
    if (cur) {
      cur.resolve(choice);
      set({ current: null });
    }
  },
}));

/** Ask the operator how to handle upload name collisions. Resolves to the
 *  chosen policy, or `null` if they cancel the whole upload. */
export function uploadConflictDialog(req: {
  count: number;
  total: number;
  sampleNames: string[];
}): Promise<UploadConflict | null> {
  return useConflictStore.getState().open(req);
}
