/** Pure model behind FolderTree: builds the hierarchy from the flat
 *  all-folders response, does diacritic-insensitive filtering, and the
 *  ancestor math behind auto-reveal. Kept free of React/DOM so it can be
 *  unit-tested directly. */

export interface FolderNode {
  name: string;
  path: string;
  /** Whatever count makes sense next to the folder name (track/file count). */
  badge?: number | string | null;
}

export interface FolderIndex {
  byPath: Map<string, FolderNode>;
  /** Parent path ("" = root) → children, sorted like a file manager. */
  childrenOf: Map<string, FolderNode[]>;
}

/** Case/diacritic-insensitive and numeric-aware, so "Disc 2" sorts before
 *  "Disc 10" and "Žánry" doesn't sink below "z". */
const collator = new Intl.Collator(undefined, { sensitivity: "base", numeric: true });

export function parentPath(path: string): string {
  const i = path.lastIndexOf("/");
  return i === -1 ? "" : path.slice(0, i);
}

/** Ancestor chain of `path`, outermost first, excluding `path` itself:
 *  "a/b/c" → ["a", "a/b"]. */
export function ancestorsOf(path: string): string[] {
  const out: string[] = [];
  let i = path.indexOf("/");
  while (i !== -1) {
    out.push(path.slice(0, i));
    i = path.indexOf("/", i + 1);
  }
  return out;
}

export function buildFolderIndex(folders: FolderNode[]): FolderIndex {
  const byPath = new Map<string, FolderNode>();
  const childrenOf = new Map<string, FolderNode[]>();
  for (const f of folders) {
    byPath.set(f.path, f);
    const parent = parentPath(f.path);
    const siblings = childrenOf.get(parent);
    if (siblings) siblings.push(f);
    else childrenOf.set(parent, [f]);
  }
  for (const siblings of childrenOf.values()) {
    siblings.sort((a, b) => collator.compare(a.name, b.name));
  }
  return { byPath, childrenOf };
}

/** Match-fold: NFKD accent strip + lowercase, same approach as `slugify`
 *  but preserving every character class ("Dvořák" → "dvorak"). */
export function foldName(s: string): string {
  return s
    .normalize("NFKD")
    .replace(/\p{M}/gu, "")
    .toLowerCase();
}

/** Find `query` in `name` under foldName semantics. Returns the matched
 *  range in ORIGINAL code-unit indices (for highlight slicing), or null.
 *  Folding can change string length (é is 2 code units NFKD-stripped to 1
 *  char), so the fold runs char-by-char with an index map back. */
export function foldedMatch(name: string, query: string): [number, number] | null {
  const q = foldName(query);
  if (q === "") return null;
  let folded = "";
  const origIndex: number[] = [];
  let cursor = 0;
  for (const ch of name) {
    const f = foldName(ch);
    for (let k = 0; k < f.length; k += 1) origIndex.push(cursor);
    folded += f;
    cursor += ch.length;
  }
  const at = folded.indexOf(q);
  if (at === -1) return null;
  const start = origIndex[at];
  const endFolded = at + q.length;
  const end = endFolded < origIndex.length ? origIndex[endFolded] : name.length;
  return [start, end];
}

export interface FilterResult {
  /** Paths to render: matches plus every ancestor of a match (context). */
  visible: Set<string>;
  /** Paths whose own name matched (highlighted; drives the match count). */
  matches: Set<string>;
}

export function filterFolders(folders: FolderNode[], query: string): FilterResult {
  const visible = new Set<string>();
  const matches = new Set<string>();
  for (const f of folders) {
    if (foldedMatch(f.name, query) !== null) {
      matches.add(f.path);
      visible.add(f.path);
      for (const a of ancestorsOf(f.path)) visible.add(a);
    }
  }
  return { visible, matches };
}
