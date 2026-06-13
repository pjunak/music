/** Drag-drop helpers for the Library upload zone.
 *
 *  The browser's DataTransfer surface gives us two ways to read a drop:
 *  - `dataTransfer.files` is a flat FileList — no folder shape, no
 *    relative paths. A dropped folder appears as zero files in this list.
 *  - `dataTransfer.items[*].webkitGetAsEntry()` returns a FileSystemEntry
 *    that knows whether it's a file or a directory and lets us recurse.
 *
 *  We use the second path whenever it's available (Firefox / Chrome /
 *  Edge / Safari all support it, with the webkit-prefixed name still
 *  being the universally-working spelling) so folder drops Just Work.
 *  The DM can grab an "Skyrim/" or "AlbumName/" folder, drop it on the
 *  upload zone, and the relative directory structure is preserved as
 *  subfolders under the currently-selected destination.
 */

/** Audio extensions we treat as "actually upload-able". Drops can contain
 *  non-audio files (cover art, .nfo, .txt) — silently skipping them keeps
 *  the music library clean instead of polluting it with unindexable junk. */
const AUDIO_EXTENSIONS = new Set([
  ".mp3",
  ".flac",
  ".ogg",
  ".opus",
  ".m4a",
  ".aac",
  ".wav",
  ".wma",
]);

export interface CollectedFile {
  /** Path of the file relative to the drop root. For a folder drop, looks
   *  like `MyAlbum/Disc 1/song1.mp3`. For a flat file drop, just the
   *  basename (`song1.mp3`). */
  relativePath: string;
  file: File;
}

/** True if the filename has an extension we'll upload. Case-insensitive. */
export function isAudioFile(name: string): boolean {
  const dot = name.lastIndexOf(".");
  if (dot < 0) return false;
  return AUDIO_EXTENSIONS.has(name.slice(dot).toLowerCase());
}

/** Resolve a FileSystemFileEntry into a File. The entry API is callback-
 *  based; wrap it in a Promise so the recursion can use async/await. */
function readFile(entry: FileSystemFileEntry): Promise<File> {
  return new Promise((resolve, reject) => {
    entry.file(resolve, reject);
  });
}

/** Read one batch of children from a directory reader. Chrome returns
 *  entries in batches of ~100 — empty batch means we've drained the dir. */
function readEntries(
  reader: FileSystemDirectoryReader,
): Promise<FileSystemEntry[]> {
  return new Promise((resolve, reject) => {
    reader.readEntries(resolve, reject);
  });
}

async function collectFromEntry(
  entry: FileSystemEntry,
  prefix: string,
  out: CollectedFile[],
): Promise<void> {
  if (entry.isFile) {
    const fileEntry = entry as FileSystemFileEntry;
    const file = await readFile(fileEntry);
    out.push({ relativePath: prefix + file.name, file });
    return;
  }
  if (entry.isDirectory) {
    const dirEntry = entry as FileSystemDirectoryEntry;
    const reader = dirEntry.createReader();
    // Drain in batches until the reader returns an empty array — that's
    // the "no more entries" signal per the spec. A naive single call can
    // miss everything past the first ~100 entries on Chrome.
    let batch = await readEntries(reader);
    while (batch.length > 0) {
      for (const child of batch) {
        await collectFromEntry(child, `${prefix}${dirEntry.name}/`, out);
      }
      batch = await readEntries(reader);
    }
  }
}

/** Expand a DataTransferItemList into a flat list of files with relative
 *  paths preserving the dropped folder structure. Non-audio files are
 *  filtered out. Returns an empty list if the browser doesn't expose the
 *  entry API — the caller should fall back to `dataTransfer.files` then. */
/** Pull FileSystemEntry handles out of a DataTransferItemList.
 *
 *  MUST be called *synchronously* inside the drop event handler, before
 *  any `await`. The DataTransfer (and the entry objects derived from it)
 *  are only valid during the synchronous portion of event dispatch — once
 *  the handler yields, Firefox in particular neuters them and the later
 *  async reads return nothing. So the caller captures entries here first,
 *  then hands the array to `collectEntries` for the async walk. */
export function entriesFromItems(items: DataTransferItemList): FileSystemEntry[] {
  const entries: FileSystemEntry[] = [];
  for (let i = 0; i < items.length; i++) {
    const item = items[i];
    if (item.kind !== "file") continue;
    // webkitGetAsEntry is the cross-browser spelling; some libs check for
    // unprefixed `getAsEntry` too but webkitGetAsEntry is the one that
    // actually works everywhere right now.
    const entry = item.webkitGetAsEntry?.();
    if (entry !== null && entry !== undefined) entries.push(entry);
  }
  return entries;
}

/** Walk pre-captured FileSystemEntry handles into a flat list of audio
 *  files with their relative paths. Async (directory reads are callback-
 *  based) — but safe to run after the drop event because the entry
 *  handles were already captured synchronously via `entriesFromItems`. */
export async function collectEntries(
  entries: FileSystemEntry[],
): Promise<CollectedFile[]> {
  const out: CollectedFile[] = [];
  for (const entry of entries) {
    await collectFromEntry(entry, "", out);
  }
  return out.filter((c) => isAudioFile(c.file.name));
}

/** Group collected files by their parent directory under the drop root.
 *  Files at the top level land under "" (empty string). Used by the
 *  uploader to fire one upload per destination subfolder. */
export function groupByParent(
  collected: CollectedFile[],
): Map<string, File[]> {
  const groups = new Map<string, File[]>();
  for (const { relativePath, file } of collected) {
    const slash = relativePath.lastIndexOf("/");
    const parent = slash < 0 ? "" : relativePath.slice(0, slash);
    const existing = groups.get(parent);
    if (existing !== undefined) {
      existing.push(file);
    } else {
      groups.set(parent, [file]);
    }
  }
  return groups;
}
