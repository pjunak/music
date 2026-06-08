/** Backend slug fields (`CreateModeRequest.id` etc.) cap at 64 chars. Keep
 *  derived slugs within that so a long name never silently produces an id the
 *  create endpoint rejects with a 400. */
const MAX_SLUG_LEN = 64;

/** Derive a filesystem-safe slug from a human name — the inverse of
 *  `humaniseSlug` (PlayerView). The result always matches the backend's slug
 *  contract (`^[a-z0-9][a-z0-9_-]*$`) within `maxLength`, or is empty (callers
 *  fall back via `uniqueSlug`). Accents fold ("Café" → "cafe") and any run of
 *  other characters collapses to a single dash. */
export function slugify(name: string, maxLength = MAX_SLUG_LEN): string {
  const base = name
    .normalize("NFKD")
    .replace(/\p{M}/gu, "") // strip combining marks left by NFKD (é → e)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-") // any non-alphanumeric run → one dash
    .replace(/^-+|-+$/g, ""); // trim edge dashes — a slug must start alnum
  if (base.length <= maxLength) return base;
  return base.slice(0, maxLength).replace(/-+$/g, ""); // re-trim a dash split
}

/** A collision-free slug derived from `name`: `slugify(name)`, suffixed
 *  `-2`, `-3`, … if it clashes with an entry in `existing` (with room reserved
 *  within `maxLength` for the suffix). Falls back to `fallback` when the name
 *  slugifies to empty (e.g. only punctuation/emoji), so the result is always a
 *  valid, non-empty slug. This is the one place authoring create-flows mint an
 *  id, so the operator only ever types a name. */
export function uniqueSlug(
  name: string,
  existing: Iterable<string>,
  fallback = "item",
  maxLength = MAX_SLUG_LEN,
): string {
  const taken = existing instanceof Set ? existing : new Set(existing);
  const root = slugify(name, maxLength) || fallback;
  if (!taken.has(root)) return root;
  for (let n = 2; ; n += 1) {
    const suffix = `-${n}`;
    const stem = slugify(root, maxLength - suffix.length) || fallback;
    const candidate = `${stem}${suffix}`;
    if (!taken.has(candidate)) return candidate;
  }
}
