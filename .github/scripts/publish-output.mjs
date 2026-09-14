import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const archive = "music-output-linux-x86_64.tar.gz";
const hash = bytes => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

// Only the release job supplies this temporary job token. Downloads need no token.
export async function publishOutput({ repository, sha, ref, directory, token, request = fetch }) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(repository ?? "") || !/^[a-f0-9]{40}$/.test(sha ?? "") || ref !== "refs/heads/main" || !token) {
    throw new Error("Output publication requires a main commit, repository and job token");
  }
  const assets = await Promise.all([archive, "SHA256SUMS", "REVISION"].map(async name => ({ name, bytes: await readFile(resolve(directory, name)) })));
  if (assets[2].bytes.toString() !== `${sha}\n` || assets[1].bytes.toString() !== `${hash(assets[0].bytes).slice(7)}  ${archive}\n`) {
    throw new Error("Output revision or archive checksum does not match");
  }
  const api = `https://api.github.com/repos/${repository}`;
  async function call(url, method = "GET", body, binary = false) {
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" || !["api.github.com", "uploads.github.com"].includes(parsed.hostname)) throw new Error("Unexpected GitHub API host");
    const response = await request(url, {
      method, redirect: "error", signal: AbortSignal.timeout(120_000),
      headers: { Authorization: `Bearer ${token}`, Accept: "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28",
        ...(body === undefined ? {} : { "Content-Type": binary ? "application/octet-stream" : "application/json" }) },
      ...(body === undefined ? {} : { body: binary ? body : JSON.stringify(body) }),
    });
    if (method === "GET" && response.status === 404) return null;
    if (!response.ok) throw new Error(`GitHub ${method} failed (${response.status})`);
    return response.json();
  }
  const main = await call(`${api}/commits/main`);
  if (main?.sha !== sha) return "Skipped superseded main revision";
  const tag = `music-output-${sha}`;
  let release = await call(`${api}/releases/tags/${tag}`);
  if (!release) release = await call(`${api}/releases`, "POST", {
    tag_name: tag, target_commitish: sha, name: `Music output ${sha.slice(0, 7)}`, draft: true, make_latest: "false",
    body: `Tested Linux x86-64 speaker client for Debian 13 / Ubuntu 24.04 or newer. Requires mpv.\n\nSource: ${sha}\n\nInstallation and rollback: https://github.com/pjunak/dnd-table#music-output\n\nUpdates are installed by the device owner. Publication does not update any device. Automated checks cover packaging and service recovery; physical audio acceptance remains device-specific.`,
  });
  if (release.tag_name !== tag || release.target_commitish !== sha || !Array.isArray(release.assets)) throw new Error("Release source does not match");
  for (const asset of assets) {
    const existing = release.assets.find(item => item.name === asset.name);
    if (existing) {
      if (existing.digest !== hash(asset.bytes)) throw new Error(`Published bytes differ: ${asset.name}; refusing replacement`);
      continue;
    }
    if (!release.draft) throw new Error(`Published release is incomplete: ${asset.name}`);
    const upload = new URL(release.upload_url.replace(/\{.*$/, ""));
    upload.searchParams.set("name", asset.name);
    const uploaded = await call(upload.href, "POST", asset.bytes, true);
    if (uploaded.digest !== hash(asset.bytes)) throw new Error(`Upload checksum mismatch: ${asset.name}`);
  }
  if (release.draft) {
    const current = await call(`${api}/commits/main`);
    await call(`${api}/releases/${release.id}`, "PATCH", { draft: false, make_latest: current?.sha === sha ? "true" : "false" });
  }
  return `Published ${tag}`;
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  publishOutput({ repository: process.env.GITHUB_REPOSITORY, sha: process.env.GITHUB_SHA, ref: process.env.GITHUB_REF,
    directory: process.argv[2] ?? "dist/music-output", token: process.env.GH_TOKEN }).then(console.log).catch(error => {
    console.error(error.message); process.exitCode = 1;
  });
}
