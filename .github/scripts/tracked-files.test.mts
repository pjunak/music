import assert from "node:assert/strict";
import test from "node:test";

import { forbiddenTrackedPaths } from "./tracked-files.mts";

await test("build output, databases, media, env files and keys are rejected", () => {
  assert.deepEqual(forbiddenTrackedPaths([
    "README.md",
    "crates/music-storage/src/schema.rs",
    ".env.example",
    "target/debug/music-server",
    "frontend/node_modules/react/index.js",
    "app.db",
    "fixtures/private.flac",
    "config/.env",
    "secrets/server.pem",
  ]), [
    "app.db",
    "config/.env",
    "fixtures/private.flac",
    "frontend/node_modules/react/index.js",
    "secrets/server.pem",
    "target/debug/music-server",
  ]);
});
