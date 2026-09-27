import { readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";

const manifestUrl = new URL("../dist/.vite/manifest.json", import.meta.url);
const manifest: unknown = JSON.parse(readFileSync(manifestUrl, "utf8"));
if (typeof manifest !== "object" || manifest === null || Array.isArray(manifest)) {
  throw new Error("frontend build manifest must be an object");
}
const entries = Object.values(manifest).filter(
  (chunk: unknown): chunk is { isEntry: true; file: string } =>
    typeof chunk === "object" && chunk !== null &&
    "isEntry" in chunk && chunk.isEntry === true &&
    "file" in chunk && typeof chunk.file === "string",
);

if (entries.length !== 1) {
  throw new Error(`expected one frontend entry chunk, found ${entries.length}`);
}

const entryUrl = new URL(`../dist/${entries[0].file}`, import.meta.url);
const entryBytes = statSync(entryUrl).size;
const gzipBytes = gzipSync(readFileSync(entryUrl)).byteLength;
const maxEntryBytes = 450_000;

console.log(
  `Entry bundle: ${(entryBytes / 1000).toFixed(2)} kB ` +
    `(${(gzipBytes / 1000).toFixed(2)} kB gzip; ` +
    `${(maxEntryBytes / 1000).toFixed(0)} kB budget)`,
);

if (entryBytes > maxEntryBytes) {
  throw new Error(
    "frontend entry bundle exceeded its budget; add or restore a lazy feature boundary",
  );
}
