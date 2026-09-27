import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test, { type TestContext } from "node:test";
import {
  publishOutput,
  type GitHubRequest,
  type PublishOutputOptions,
} from "./publish-output.mts";

const sha = "a".repeat(40);
const archive = "music-output-linux-x86_64.tar.gz";
const digest = (bytes: Uint8Array) => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;

interface MockAsset {
  name: string | null;
  digest: string;
}

interface MockRelease {
  id: number;
  tag_name: string;
  target_commitish: string;
  name: string;
  draft: boolean;
  make_latest: string;
  body: string;
  assets: MockAsset[];
  upload_url: string;
}

interface MockState {
  main: string;
  release: MockRelease | null;
  calls: Array<[string, string]>;
  failUpload: boolean;
}

function jsonBody(body: string | ArrayBuffer | undefined): Record<string, unknown> {
  if (typeof body !== "string") throw new Error("Expected a JSON request body");
  return JSON.parse(body) as Record<string, unknown>;
}

async function fixture(t: TestContext) {
  const directory = await mkdtemp(join(tmpdir(), "music-release-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const bytes = Buffer.from("verified binary archive");
  const files: ReadonlyArray<readonly [string, string | Uint8Array]> = [
    [archive, bytes],
    ["SHA256SUMS", `${digest(bytes).slice(7)}  ${archive}\n`],
    ["REVISION", `${sha}\n`],
  ];
  for (const [name, value] of files) await writeFile(join(directory, name), value);
  const state: MockState = { main: sha, release: null, calls: [], failUpload: false };
  const request: GitHubRequest = async (url, options) => {
    state.calls.push([url, options.method]);
    const path = new URL(url).pathname;
    let data: unknown;
    if (path.endsWith("/commits/main")) data = { sha: state.main };
    else if (path.includes("/releases/tags/")) return new Response(JSON.stringify(state.release), { status: state.release ? 200 : 404 });
    else if (options.method === "PATCH") {
      assert.ok(state.release);
      Object.assign(state.release, jsonBody(options.body));
      data = state.release;
    }
    else if (path.endsWith("/releases")) {
      state.release = {
        ...jsonBody(options.body),
        id: 1,
        assets: [],
        upload_url: "https://uploads.github.com/repos/pjunak/music/releases/1/assets{?name,label}",
      } as unknown as MockRelease;
      data = state.release;
    } else if (path.endsWith("/assets")) {
      if (state.failUpload) return new Response("failed", { status: 500 });
      assert.ok(options.body instanceof ArrayBuffer);
      const uploadedBytes = new Uint8Array(options.body);
      const uploadedAsset: MockAsset = {
        name: new URL(url).searchParams.get("name"),
        digest: digest(uploadedBytes),
      };
      data = uploadedAsset;
      assert.ok(state.release);
      state.release.assets.push(uploadedAsset);
    } else throw new Error(`Unexpected request ${url}`);
    return Response.json(data);
  };
  const options: PublishOutputOptions = {
    repository: "pjunak/music",
    sha,
    ref: "refs/heads/main",
    directory,
    token: "job-token",
    request,
  };
  return { state, options, directory };
}

await test("publishes all verified assets before making the commit latest; reruns do not replace them", async t => {
  const { state, options } = await fixture(t);
  await publishOutput(options);
  assert.ok(state.release);
  assert.equal(state.release.draft, false);
  assert.equal(state.release.make_latest, "true");
  assert.equal(state.release.assets.length, 3);
  state.calls = [];
  await publishOutput(options);
  assert.ok(state.release);
  assert.ok(state.calls.every(([, method]) => method === "GET"));
});
await test("corrupt package or wrong revision makes no network changes", async t => {
  const { state, options, directory } = await fixture(t);
  await writeFile(join(directory, archive), "corrupted");
  await assert.rejects(publishOutput(options), /checksum/);
  assert.equal(state.calls.length, 0);
});
await test("superseded or non-main commits cannot promote a download", async t => {
  const { state, options } = await fixture(t);
  state.main = "b".repeat(40);
  assert.match(await publishOutput(options), /Skipped/);
  assert.equal(state.release, null);
  await assert.rejects(publishOutput({ ...options, ref: "refs/heads/topic" }), /main commit/);
});
await test("failed uploads stay draft and can resume without replacing completed assets", async t => {
  const { state, options } = await fixture(t);
  state.failUpload = true;
  await assert.rejects(publishOutput(options), /500/);
  assert.ok(state.release);
  assert.equal(state.release.draft, true);
  state.failUpload = false;
  await publishOutput(options);
  assert.ok(state.release);
  assert.equal(state.release.draft, false);
});
await test("published bytes cannot be replaced on a rerun", async t => {
  const { state, options } = await fixture(t);
  await publishOutput(options);
  assert.ok(state.release);
  const firstAsset = state.release.assets[0];
  assert.ok(firstAsset);
  firstAsset.digest = "sha256:" + "0".repeat(64);
  await assert.rejects(publishOutput(options), /refusing replacement/);
});
await test("a newer main arriving during upload does not get replaced as latest", async t => {
  const { state, options } = await fixture(t);
  const request = options.request;
  assert.ok(request);
  options.request = async (url, requestOptions) => {
    const result = await request(url, requestOptions);
    if (state.release?.assets.length === 3) state.main = "b".repeat(40);
    return result;
  };
  await publishOutput(options);
  assert.ok(state.release);
  assert.equal(state.release.make_latest, "false");
});
