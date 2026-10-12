import { useCallback, useState } from "react";

import { Breadcrumb } from "@/components/Breadcrumb";
import type { BreadcrumbItem } from "@/components/Breadcrumb";
import { confirmDialog } from "@/components/confirmDialog";
import { FolderPickerModal } from "@/components/FolderPickerModal";
import { IconButton } from "@/components/IconButton";
import { EditIcon, MoveIcon, PlayIcon, PlusIcon, ShuffleIcon, TrashIcon } from "@/components/icons";
import { inputDialog } from "@/components/inputDialog";
import { libraryApi, sfxApi } from "@/core/api";
import { toast } from "@/core/toast";
import { wsClient } from "@/core/ws";

import { musicAllFolders, sfxAllFolders } from "./shared";
import type { Root } from "./shared";

export function FolderHeader({
  path,
  root,
  onPathSelect,
}: {
  path: string;
  root: Root;
  onPathSelect: (p: string) => void;
}) {
  const segments = path === "" ? [] : path.split("/");
  // Root segment is always present; subsequent segments are the path
  // pieces. The Breadcrumb component renders the last item as static
  // "you are here" automatically — so even though we attach onClick to
  // the deepest item, it won't fire.
  const items: BreadcrumbItem[] = [
    {
      label: root === "music" ? "Music" : "SFX",
      onClick: () => onPathSelect(""),
      title: root === "music" ? "Go to music root" : "Go to SFX root",
    },
    ...segments.map((name, i) => {
      const segmentPath = segments.slice(0, i + 1).join("/");
      return {
        label: name,
        onClick: () => onPathSelect(segmentPath),
        title: segmentPath,
      };
    }),
  ];

  // Load this folder (recursive) into the ambient queue. "" plays the whole
  // library. "Shuffle" flips shuffle to random first so the load is randomised;
  // both respect Continue (∞) for what happens once the folder is exhausted.
  const folderLabel = path === "" ? "all music" : segments[segments.length - 1];
  function playFolder(shuffled: boolean) {
    if (shuffled) wsClient.send({ type: "ambient_set_shuffle", shuffle: "random" });
    wsClient.send({ type: "ambient_play_folder", path });
  }

  return (
    <div className="folder-header">
      <Breadcrumb items={items} />
      {root === "music" ? (
        <div className="folder-header-actions">
          <IconButton
            label={`Play ${folderLabel}`}
            icon={<PlayIcon />}
            variant="primary"
            onClick={() => playFolder(false)}
          />
          <IconButton
            label={`Shuffle ${folderLabel}`}
            icon={<ShuffleIcon />}
            onClick={() => playFolder(true)}
          />
        </div>
      ) : null}
    </div>
  );
}

export function FolderActions({
  root,
  selectedPath,
  onChanged,
  onPathReset,
}: {
  root: Root;
  selectedPath: string;
  onChanged: () => void;
  onPathReset: () => void;
}) {
  const [moveOpen, setMoveOpen] = useState(false);
  const [moveBusy, setMoveBusy] = useState(false);

  const loadAll = useCallback(
    () => (root === "music" ? musicAllFolders() : sfxAllFolders()),
    [root],
  );

  async function newFolder() {
    const name = await inputDialog({
      title: "New folder",
      body: `Will be created under ${selectedPath ? `"${selectedPath}"` : "the root"}.`,
      label: "Folder name",
      placeholder: "e.g. Skyrim",
      confirmLabel: "Create",
      // Reject path separators — operators create *one* folder at a time;
      // multi-level paths are unintuitive in a single-shot prompt.
      validate: (v) =>
        v.includes("/") || v.includes("\\") ? "No slashes — pick a nested folder, then create inside it." : null,
    });
    if (name === null) return;
    const target = selectedPath ? `${selectedPath}/${name}` : name;
    try {
      if (root === "music") await libraryApi.createFolder(target);
      else await sfxApi.createFolder(target);
      toast.success("Folder created", target);
      onChanged();
    } catch (e) {
      toast.error("Create failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function renameFolder() {
    if (!selectedPath) {
      toast.info("Pick a folder first");
      return;
    }
    const parent = selectedPath.split("/").slice(0, -1).join("/");
    const current = selectedPath.split("/").pop() ?? selectedPath;
    const name = await inputDialog({
      title: "Rename folder",
      body: `Renaming "${selectedPath}". Use Move… to change its parent.`,
      label: "New name",
      initial: current,
      confirmLabel: "Rename",
      validate: (v) =>
        v.includes("/") || v.includes("\\")
          ? "No slashes — use Move… to change the parent folder."
          : null,
    });
    if (name === null || name === current) return;
    const target = parent ? `${parent}/${name}` : name;
    try {
      if (root === "music") await libraryApi.renameFolder(selectedPath, target);
      else await sfxApi.renameFolder(selectedPath, target);
      toast.success("Folder renamed", target);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Rename failed", e instanceof Error ? e.message : undefined);
    }
  }

  async function moveFolder(destParent: string) {
    const name = selectedPath.split("/").pop() ?? selectedPath;
    const target = destParent === "" ? name : `${destParent}/${name}`;
    if (target === selectedPath) {
      setMoveOpen(false);
      return;
    }
    setMoveBusy(true);
    try {
      if (root === "music") await libraryApi.renameFolder(selectedPath, target);
      else await sfxApi.renameFolder(selectedPath, target);
      toast.success("Folder moved", target);
      setMoveOpen(false);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Move failed", e instanceof Error ? e.message : undefined);
    } finally {
      setMoveBusy(false);
    }
  }

  async function deleteFolder() {
    if (!selectedPath) {
      toast.info("Pick a folder first");
      return;
    }
    const ok = await confirmDialog({
      title: `Delete "${selectedPath}"?`,
      body:
        "If the folder isn't empty, everything inside it will be removed too. This can't be undone.",
      confirmLabel: "Delete recursively",
      tone: "danger",
    });
    if (!ok) return;
    try {
      if (root === "music")
        await libraryApi.deleteFolder(selectedPath, true);
      else await sfxApi.deleteFolder(selectedPath, true);
      toast.success("Folder deleted", selectedPath);
      onChanged();
      onPathReset();
    } catch (e) {
      toast.error("Delete failed", e instanceof Error ? e.message : undefined);
    }
  }

  return (
    <>
      <div className="folder-actions">
        <IconButton
          label="Create a folder"
          icon={<PlusIcon />}
          onClick={() => void newFolder()}
        >
          New
        </IconButton>
        <IconButton
          label="Rename the selected folder"
          icon={<EditIcon />}
          disabled={!selectedPath}
          onClick={() => void renameFolder()}
        >
          Rename
        </IconButton>
        <IconButton
          label="Move the selected folder (and everything in it) into another folder"
          icon={<MoveIcon />}
          disabled={!selectedPath}
          onClick={() => setMoveOpen(true)}
        >
          Move…
        </IconButton>
        <IconButton
          label="Delete the selected folder and everything in it"
          icon={<TrashIcon />}
          variant="danger"
          className="folder-actions-delete"
          disabled={!selectedPath}
          onClick={() => void deleteFolder()}
        >
          Delete
        </IconButton>
      </div>
      {moveOpen ? (
        <FolderPickerModal
          title={`Move "${selectedPath}"`}
          body="Pick the destination parent folder. The folder and everything inside it moves together."
          loadAll={loadAll}
          busy={moveBusy}
          disableDest={(dest) =>
            dest === selectedPath || dest.startsWith(`${selectedPath}/`)
              ? "Can't move a folder into itself or one of its subfolders."
              : dest === selectedPath.split("/").slice(0, -1).join("/")
                ? "The folder is already here."
                : null
          }
          onCancel={() => setMoveOpen(false)}
          onConfirm={(dest) => void moveFolder(dest)}
        />
      ) : null}
    </>
  );
}
