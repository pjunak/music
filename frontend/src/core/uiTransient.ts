import { create } from "zustand";

/** Transient (non-persisted) UI state. Anything in here resets to its
 *  initial value on full page reload — open/closed flags for modals,
 *  ephemeral panels, etc. Sibling to `uiStore` which persists preferences
 *  to localStorage. */

interface UiTransientStore {
  /** Whether the keyboard-shortcut sheet is currently open. */
  shortcutSheetOpen: boolean;
  setShortcutSheetOpen: (open: boolean) => void;
}

export const useUiTransient = create<UiTransientStore>()((set) => ({
  shortcutSheetOpen: false,
  setShortcutSheetOpen: (open) => set({ shortcutSheetOpen: open }),
}));
