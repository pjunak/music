import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const archive = "music-output-linux-x86_64.tar.gz";
const hash = (bytes: Uint8Array) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

interface GitHubRequestOptions {
  method: string;
  redirect: "error";
  signal: AbortSignal;
  headers: Record<string, string>;
  body?: string | ArrayBuffer;
}

export type GitHubRequest = (url: string, options: GitHubRequestOptions) => Promise<Response>;

export interface PublishOutputOptions {
  repository?: string | undefined;
  sha?: string | undefined;
  ref?: string | undefined;
  directory: string;
  token?: string | undefined;
  request?: GitHubRequest | undefined;
}

interface GitHubCommit {
  sha?: string;
}

interface GitHubAsset {
  name: string;
  digest?: string;
}

interface GitHubRelease {
  id: number;
  tag_name: string;
  target_commitish: string;
  assets: GitHubAsset[];
  upload_url: string;
  draft: boolean;
}

// Only the release job supplies this temporary job token. Downloads need no token.
export async function publishOutput({
  repository,
  sha,
  ref,
  directory,
  token,
  request = fetch,
}: PublishOutputOptions): Promise<string> {
  if (typeof repository !== "string" || !/^[\w.-]+\/[\w.-]+$/.test(repository)
    || typeof sha !== "string" || !/^[a-f0-9]{40}$/.test(sha)
    || ref !== "refs/heads/main" || typeof token !== "string" || !token) {
    throw new Error("Output publication requires a main commit, repository and job token");
  }
  const loadAsset = async (name: string) => ({ name, bytes: await readFile(resolve(directory, name)) });
  const [archiveAsset, sumsAsset, revisionAsset] = await Promise.all([
    loadAsset(archive),
    loadAsset("SHA256SUMS"),
    loadAsset("REVISION"),
  ]);
  const assets = [archiveAsset, sumsAsset, revisionAsset];
  if (revisionAsset.bytes.toString() !== `${sha}\n`
    || sumsAsset.bytes.toString() !== `${hash(archiveAsset.bytes).slice(7)}  ${archive}\n`) {
    throw new Error("Output revision or archive checksum does not match");
  }
  const api = `https://api.github.com/repos/${repository}`;
  async function call<T>(
    url: string,
    method = "GET",
    body?: Record<string, unknown> | Uint8Array,
    binary = false,
  ): Promise<T | null> {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" || !["api.github.com", "uploads.github.com"].includes(parsed.hostname)) throw new Error("Unexpected GitHub API host");
    let requestBody: string | ArrayBuffer | undefined;
    if (body !== undefined) {
      if (binary) {
        if (!(body instanceof Uint8Array)) throw new Error("Binary GitHub requests require byte content");
        requestBody = Uint8Array.from(body).buffer;
      } else {
        requestBody = JSON.stringify(body);
      }
    }
    const response = await request(url, {
      method, redirect: "error", signal: AbortSignal.timeout(120_000),
      headers: { Authorization: `Bearer ${token}`, Accept: "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
        ...(body === undefined ? {} : { "Content-Type": binary ? "application/octet-stream" : "application/json" }) },
      ...(requestBody === undefined ? {} : { body: requestBody }),
    });
    if (method === "GET" && response.status === 404) return null;
    if (!response.ok) throw new Error(`GitHub ${method} failed (${response.status})`);
    return await response.json() as T;
  }
  const main = await call<GitHubCommit>(`${api}/commits/main`);
  if (main?.sha !== sha) return "Skipped superseded main revision";
  const tag = `music-output-${sha}`;
  let release = await call<GitHubRelease>(`${api}/releases/tags/${tag}`);
  if (!release) release = await call<GitHubRelease>(`${api}/releases`, "POST", {
    tag_name: tag, target_commitish: sha, name: `Music output ${sha.slice(0, 7)}`, draft: true, make_latest: "false",
    body: `Tested Linux x86-64 speaker client for Debian 13 / Ubuntu 24.04 or newer. Requires mpv.\n\nSource: ${sha}\n\nInstallation and rollback: https://github.com/pjunak/dnd-table#music-output\n\nUpdates are installed by the device owner. Publication does not update any device. Automated checks cover packaging and service recovery; physical audio acceptance remains device-specific.`,
  });
  if (!release || release.tag_name !== tag || release.target_commitish !== sha || !Array.isArray(release.assets)) throw new Error("Release source does not match");
  for (const asset of assets) {
    const existing = release.assets.find(item => item.name === asset.name);
    if (existing) {
      if (existing.digest !== hash(asset.bytes)) throw new Error(`Published bytes differ: ${asset.name}; refusing replacement`);
      continue;
    }
    if (!release.draft) throw new Error(`Published release is incomplete: ${asset.name}`);
    const upload = new URL(release.upload_url.replace(/\{.*$/, ""));
    upload.searchParams.set("name", asset.name);
    const uploaded = await call<GitHubAsset>(upload.href, "POST", asset.bytes, true);
    if (uploaded?.digest !== hash(asset.bytes)) throw new Error(`Upload checksum mismatch: ${asset.name}`);
  }
  if (release.draft) {
    const current = await call<GitHubCommit>(`${api}/commits/main`);
    await call<GitHubRelease>(`${api}/releases/${release.id}`, "PATCH", { draft: false, make_latest: current?.sha === sha ? "true" : "false" });
  }
  return `Published ${tag}`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  publishOutput({ repository: process.env.GITHUB_REPOSITORY, sha: process.env.GITHUB_SHA, ref: process.env.GITHUB_REF,
    directory: process.argv[2] ?? "dist/music-output", token: process.env.GH_TOKEN }).then(console.log).catch(error => {
    console.error(error instanceof Error ? error.message : String(error)); process.exitCode = 1;
  });
}
