import { create } from "zustand";

/** Promise-based input-dialog API. Sibling of confirmDialog — same shape,
 *  same fast-refresh constraint (store + helper in a .ts file so InputDialogHost.tsx
 *  can export only its component). Resolves with the entered string on submit,
 *  or `null` on cancel / Escape / backdrop click. */

interface InputRequest {
  title: string;
  /** Label shown above the input. */
  label?: string;
  /** Body text shown above the input (richer than a one-line label). */
  body?: string;
  /** Pre-filled value. */
  initial?: string;
  /** Placeholder when the input is empty. */
  placeholder?: string;
  /** HTML pattern attribute for inline browser validation. */
  pattern?: string;
  /** Tooltip explaining the pattern when validation fails. */
  patternHint?: string;
  /** Custom validator. Return a non-empty string to show as an error and block submit. */
  validate?: (value: string) => string | null;
  /** Submit button label. Defaults to "OK". */
  confirmLabel?: string;
  cancelLabel?: string;
  /** Trim whitespace from the resolved value. Default true — folder/id slugs
   *  almost never want trailing spaces; opt out for things like display names
   *  where leading/trailing space might be intentional. */
  trim?: boolean;
  /** True to forbid resolving an empty string. Default true. */
  required?: boolean;
  resolve: (value: string | null) => void;
}

interface InputStore {
  current: InputRequest | null;
  open: (req: Omit<InputRequest, "resolve">) => Promise<string | null>;
  resolve: (value: string | null) => void;
}

export const useInputStore = create<InputStore>()((set, get) => ({
  current: null,
  open: (req) =>
    new Promise<string | null>((resolve) => {
      const prev = get().current;
      if (prev) prev.resolve(null);
      set({ current: { ...req, resolve } });
    }),
  resolve: (value) => {
    const cur = get().current;
    if (cur) {
      cur.resolve(value);
      set({ current: null });
    }
  },
}));

export function inputDialog(req: Omit<InputRequest, "resolve">): Promise<string | null> {
  return useInputStore.getState().open(req);
}
