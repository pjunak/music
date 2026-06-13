import { create } from "zustand";

/** Transient (non-persisted) UI state. Anything in here resets to its
 *  initial value on full page reload — open/closed flags for modals,
 *  ephemeral panels, etc. Sibling to `uiStore` which persists preferences
 *  to localStorage. */

interface UiTransientStore {
  /** Whether the keyboard-shortcut sheet is currently open. */
  shortcutSheetOpen: boolean;
  setShortcutSheetOpen: (open: boolean) => void;

  /** Whether the sign-in modal is open. Login is an overlay, not a route, so
   *  reaching a protected area while signed out never navigates away — the
   *  modal opens in place and closes itself on success, leaving the operator
   *  exactly where they were. */
  loginOpen: boolean;
  setLoginOpen: (open: boolean) => void;
}

export const useUiTransient = create<UiTransientStore>()((set) => ({
  shortcutSheetOpen: false,
  setShortcutSheetOpen: (open) => set({ shortcutSheetOpen: open }),
  loginOpen: false,
  setLoginOpen: (open) => set({ loginOpen: open }),
}));
