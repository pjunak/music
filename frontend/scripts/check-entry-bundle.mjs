import { readFileSync, statSync } from "node:fs";
import { gzipSync } from "node:zlib";

const manifestUrl = new URL("../dist/.vite/manifest.json", import.meta.url);
const manifest = JSON.parse(readFileSync(manifestUrl, "utf8"));
const entries = Object.values(manifest).filter((chunk) => chunk.isEntry);

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
