import { Suspense, useState } from "react";

import { ImportIcon } from "@/components/icons";
import { lazyNamed } from "@/core/lazyNamed";
import { usePlayerStore } from "@/core/playerStore";

import { RouteSpinner } from "./routeGuards";
import { SectionNav } from "./SectionNav";

const AuthoringImportModal = lazyNamed(
  () => import("./AuthoringImportModal"),
  (module) => module.AuthoringImportModal,
);

const AUTHORING_TABS = [
  { to: "playlists", label: "Playlists" },
  { to: "soundboards", label: "Soundboards" },
  { to: "interrupts", label: "Interrupts" },
  { to: "presets", label: "EQ Presets" },
  { to: "cues", label: "Cues" },
];

export function AuthoringShell() {
  const activeModeId = usePlayerStore((state) => state.state?.active_mode_id ?? null);
  const [importOpen, setImportOpen] = useState(false);
  const [contentRevision, setContentRevision] = useState(0);

  function imported() {
    // Remount the active child editor so it fetches the newly imported mode
    // resources immediately; the server remains the canonical source.
    setContentRevision((revision) => revision + 1);
  }

  return (
    <>
      <SectionNav
        key={contentRevision}
        ariaLabel="Authoring sections"
        items={AUTHORING_TABS}
        action={
          <button
            type="button"
            className="btn-ghost section-nav-import"
            onClick={() => setImportOpen(true)}
            disabled={activeModeId === null}
            title={
              activeModeId === null
                ? "Pick an active mode before importing"
                : "Import items from another mode"
            }
          >
            <ImportIcon aria-hidden="true" />
            Import
          </button>
        }
      />
      {importOpen && activeModeId !== null ? (
        <Suspense fallback={<RouteSpinner />}>
          <AuthoringImportModal
            open
            targetModeId={activeModeId}
            onClose={() => setImportOpen(false)}
            onImported={imported}
          />
        </Suspense>
      ) : null}
    </>
  );
}
