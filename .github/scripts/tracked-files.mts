import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const REPOSITORY_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const GENERATED_DIRECTORY = /(^|\/)(?:target|node_modules|fuzz\/artifacts|fuzz\/coverage)(\/|$)/iu;
const SENSITIVE_OR_MEDIA_FILE =
  /\.(?:aac|db|flac|key|m4a|mp3|ogg|opus|p12|pem|pfx|profraw|sqlite|sqlite3|wav|wma)$/iu;

/** Tracked paths that must never be committed: build output, databases,
 *  media, environment files and key material. */
export function forbiddenTrackedPaths(paths: readonly string[]): string[] {
  return paths
    .filter((path) => GENERATED_DIRECTORY.test(path)
      || /(?:^|\/)\.env$/iu.test(path)
      || SENSITIVE_OR_MEDIA_FILE.test(path))
    .sort();
}

function trackedPaths(): string[] {
  const result = spawnSync("git", ["ls-files", "-z"], {
    cwd: REPOSITORY_ROOT,
    encoding: "utf8",
    maxBuffer: 32 * 1_024 * 1_024,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`git ls-files failed: ${String(result.stderr).trim()}`);
  return String(result.stdout).split("\0").filter(Boolean);
}

const invokedPath = process.argv[1] ? pathToFileURL(resolve(process.argv[1])).href : "";
if (invokedPath === import.meta.url) {
  const forbidden = forbiddenTrackedPaths(trackedPaths());
  if (forbidden.length > 0) {
    process.stderr.write(`tracked files that must stay out of Git:\n${forbidden.map((path) => `- ${path}`).join("\n")}\n`);
    process.exitCode = 1;
  } else {
    process.stdout.write("no build output, databases, media or keys are tracked\n");
  }
}
