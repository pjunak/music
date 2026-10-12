import { FolderTree } from "@/components/FolderTree";
import type { TreeFolder } from "@/components/FolderTree";
import { LibrarySidebarRail } from "@/components/LibrarySidebarRail";

import type { Root } from "./shared";
import { FolderActions } from "./FolderHeader";
import { FolderBand } from "./FolderUploadBand";

/** Shell shared by the Music and SFX browsers — folder tree + folder
 *  actions on the left, header + upload + error + caller-supplied content
 *  on the right. The rest of the browser-specific behaviour (which API to
 *  hit, what to render in the right pane) is the caller's responsibility,
 *  but the layout shape is defined exactly once. */
export function LibraryShell({
  rootKind,
  selectedPath,
  onPathChange,
  refreshKey,
  loadAll,
  onDropOnFolder,
  onRefresh,
  onPathReset,
  error,
  children,
}: {
  rootKind: Root;
  selectedPath: string;
  onPathChange: (p: string) => void;
  refreshKey: number;
  loadAll: () => Promise<TreeFolder[]>;
  onDropOnFolder?: (folderPath: string, payload: unknown) => void;
  onRefresh: () => void;
  onPathReset: () => void;
  error: string | null;
  children: React.ReactNode;
}) {
  return (
    <div className="library-body">
      <LibrarySidebarRail>
        <FolderTree
          selectedPath={selectedPath}
          onSelect={onPathChange}
          loadAll={loadAll}
          {...(refreshKey !== undefined ? { refreshKey } : {})}
          {...(onDropOnFolder !== undefined ? { onDropOnFolder } : {})}
        />
        <FolderActions
          root={rootKind}
          selectedPath={selectedPath}
          onChanged={onRefresh}
          onPathReset={onPathReset}
        />
      </LibrarySidebarRail>
      <section className="library-main">
        <FolderBand
          root={rootKind}
          dest={selectedPath}
          onPathSelect={onPathChange}
          onUploaded={onRefresh}
          onNavigate={onPathChange}
        />
        {error !== null ? <p className="error small">{error}</p> : null}
        {children}
      </section>
    </div>
  );
}

// --- SFX -----------------------------------------------------------------
