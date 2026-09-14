import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";
import { publishOutput } from "./publish-output.mjs";

const sha = "a".repeat(40);
const archive = "music-output-linux-x86_64.tar.gz";
const digest = bytes => `sha256:${createHash("sha256").update(bytes).digest("hex")}`;
async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), "music-release-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const bytes = Buffer.from("verified binary archive");
  for (const [name, value] of [[archive, bytes], ["SHA256SUMS", `${digest(bytes).slice(7)}  ${archive}\n`], ["REVISION", `${sha}\n`]]) await writeFile(join(directory, name), value);
  const state = { main: sha, release: null, calls: [], failUpload: false };
  const options = { repository: "pjunak/music", sha, ref: "refs/heads/main", directory, token: "job-token", request: async (url, options) => {
    state.calls.push([url, options.method]);
    const path = new URL(url).pathname;
    let data;
    if (path.endsWith("/commits/main")) data = { sha: state.main };
    else if (path.includes("/releases/tags/")) return new Response(JSON.stringify(state.release), { status: state.release ? 200 : 404 });
    else if (options.method === "PATCH") { Object.assign(state.release, JSON.parse(options.body)); data = state.release; }
    else if (path.endsWith("/releases")) {
      state.release = { ...JSON.parse(options.body), id: 1, assets: [], upload_url: "https://uploads.github.com/repos/pjunak/music/releases/1/assets{?name,label}" }; data = state.release;
    } else if (path.endsWith("/assets")) {
      if (state.failUpload) return new Response("failed", { status: 500 });
      data = { name: new URL(url).searchParams.get("name"), digest: digest(options.body) };
      state.release.assets.push(data);
    } else throw new Error(`Unexpected request ${url}`);
    return Response.json(data);
  } };
  return { state, options, directory };
}

test("publishes all verified assets before making the commit latest; reruns do not replace them", async t => {
  const { state, options } = await fixture(t);
  await publishOutput(options);
  assert.equal(state.release.draft, false);
  assert.equal(state.release.make_latest, "true");
  assert.equal(state.release.assets.length, 3);
  state.calls = [];
  await publishOutput(options);
  assert.ok(state.calls.every(([, method]) => method === "GET"));
});
test("corrupt package or wrong revision makes no network changes", async t => {
  const { state, options, directory } = await fixture(t);
  await writeFile(join(directory, archive), "corrupted");
  await assert.rejects(publishOutput(options), /checksum/);
  assert.equal(state.calls.length, 0);
});
test("superseded or non-main commits cannot promote a download", async t => {
  const { state, options } = await fixture(t);
  state.main = "b".repeat(40);
  assert.match(await publishOutput(options), /Skipped/);
  assert.equal(state.release, null);
  await assert.rejects(publishOutput({ ...options, ref: "refs/heads/topic" }), /main commit/);
});
test("failed uploads stay draft and can resume without replacing completed assets", async t => {
  const { state, options } = await fixture(t);
  state.failUpload = true;
  await assert.rejects(publishOutput(options), /500/);
  assert.equal(state.release.draft, true);
  state.failUpload = false;
  await publishOutput(options);
  assert.equal(state.release.draft, false);
});
test("published bytes cannot be replaced on a rerun", async t => {
  const { state, options } = await fixture(t);
  await publishOutput(options);
  state.release.assets[0].digest = "sha256:" + "0".repeat(64);
  await assert.rejects(publishOutput(options), /refusing replacement/);
});
test("a newer main arriving during upload does not get replaced as latest", async t => {
  const { state, options } = await fixture(t);
  const request = options.request;
  options.request = async (...args) => {
    const result = await request(...args);
    if (state.release?.assets.length === 3) state.main = "b".repeat(40);
    return result;
  };
  await publishOutput(options);
  assert.equal(state.release.make_latest, "false");
});
