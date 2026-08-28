import { createHash } from "node:crypto";
import { readFile, writeFile } from "node:fs/promises";

const DYNAMIC_TIME_KEYS = new Set([
  "added_at",
  "completed_at",
  "created_at",
  "expires_at",
  "finished_at",
  "last_scan_at",
  "last_seen",
  "position_anchored_at",
  "scan_started_at",
  "started_at",
  "updated_at",
]);
const DYNAMIC_ID_KEYS = new Set(["correlation_id", "request_id"]);
const SECURITY_HEADERS = [
  "content-security-policy",
  "x-content-type-options",
  "x-frame-options",
];

function usage() {
  return [
    "usage:",
    "  node runtime-differential.mjs capture <base-url> <fixture.flac> <output.json>",
    "  node runtime-differential.mjs compare <python.json> <rust.json> <report.md>",
  ].join("\n");
}

function sha256(bytes) {
  return createHash("sha256").update(bytes).digest("hex");
}

function normalized(value, key = "") {
  if (value === null || value === undefined) return value;
  if (DYNAMIC_TIME_KEYS.has(key)) return "<timestamp>";
  if (DYNAMIC_ID_KEYS.has(key)) return "<id>";
  if (key === "revision" && typeof value === "number") return "<revision>";
  if (Array.isArray(value)) return value.map((item) => normalized(item));
  if (typeof value === "object") {
    return Object.fromEntries(
      Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([childKey, childValue]) => [childKey, normalized(childValue, childKey)]),
    );
  }
  return value;
}

function normalizedSetCookie(response) {
  const values = response.headers.getSetCookie?.() ?? [];
  const cookies = values.length > 0 ? values : [response.headers.get("set-cookie")].filter(Boolean);
  return cookies.map((cookie) => {
    const [pair, ...rawAttributes] = cookie.split(";").map((part) => part.trim());
    const separator = pair.indexOf("=");
    const name = separator < 0 ? pair : pair.slice(0, separator);
    const cookieValue = separator < 0 ? "" : pair.slice(separator + 1);
    const attributes = {};
    for (const rawAttribute of rawAttributes) {
      const attributeSeparator = rawAttribute.indexOf("=");
      const rawName = attributeSeparator < 0 ? rawAttribute : rawAttribute.slice(0, attributeSeparator);
      const rawValue = attributeSeparator < 0 ? true : rawAttribute.slice(attributeSeparator + 1);
      const attributeName = rawName.toLowerCase();
      if (attributeName === "expires") attributes[attributeName] = "<timestamp>";
      else if (attributeName === "samesite" && typeof rawValue === "string") {
        attributes[attributeName] = rawValue.toLowerCase();
      } else attributes[attributeName] = rawValue;
    }
    if (cookieValue === "" && attributes["max-age"] === "0") delete attributes.httponly;
    return normalized({ name, value: "<token>", attributes });
  });
}

function normalizedHeaderValue(name, value) {
  if (name !== "content-type") return value;
  const mediaType = value.split(";", 1)[0].trim().toLowerCase();
  return mediaType === "audio/x-flac" ? "audio/flac" : mediaType;
}

function selectedHeaders(response, names) {
  const headers = {};
  for (const name of names) {
    if (name === "set-cookie") {
      const cookies = normalizedSetCookie(response);
      if (cookies.length > 0) headers[name] = cookies;
      continue;
    }
    const value = response.headers.get(name);
    if (value === null) continue;
    if (name === "etag") headers[name] = "<present>";
    else if (name === "last-modified") headers[name] = "<timestamp>";
    else headers[name] = normalizedHeaderValue(name, value);
  }
  return normalized(headers);
}

async function observeResponse(response, { body = "json", headers = [] } = {}) {
  const bytes = Buffer.from(await response.arrayBuffer());
  let observedBody;
  if (body === "json") {
    observedBody = bytes.length === 0 ? null : normalized(JSON.parse(bytes.toString("utf8")));
    if (response.status === 422) {
      if (!observedBody || typeof observedBody !== "object" || !("detail" in observedBody)) {
        throw new Error("HTTP 422 response did not contain the expected detail envelope");
      }
      observedBody = { detail: "<validation-error>" };
    }
  } else if (body === "hash") {
    observedBody = { bytes: bytes.length, sha256: sha256(bytes) };
  } else {
    throw new Error(`unsupported body observation: ${body}`);
  }
  return {
    status: response.status,
    headers: selectedHeaders(response, headers),
    body: observedBody,
  };
}

async function request(baseUrl, path, init, observation) {
  const response = await fetch(new URL(path, baseUrl), { redirect: "manual", ...init });
  return { response, observed: await observeResponse(response, observation) };
}

function cookiePair(response) {
  const value = response.headers.getSetCookie?.()[0] ?? response.headers.get("set-cookie");
  if (!value) throw new Error("login response did not set a session cookie");
  return value.split(";", 1)[0];
}

function jsonBody(value) {
  return {
    headers: { "content-type": "application/json" },
    body: JSON.stringify(value),
  };
}

function websocketUrl(baseUrl) {
  const url = new URL("/api/ws", baseUrl);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  return url.href;
}

async function guestWebSocketProjection(baseUrl, protocolVersion, exerciseMutation) {
  const clientId = `differential-v${protocolVersion}`;
  const registration = {
    type: "register",
    name: `Differential v${protocolVersion}`,
    client_id: clientId,
  };
  if (protocolVersion !== 1) registration.protocol_version = protocolVersion;

  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(baseUrl));
    let projection;
    let mutationError;
    let settled = false;
    let mutationSent = false;
    const timeout = setTimeout(() => fail(new Error("WebSocket differential timed out")), 5_000);

    function finish() {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      resolve(normalized({ projection, mutation_error: mutationError ?? null }));
    }

    function fail(error) {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      if (socket.readyState === WebSocket.OPEN) socket.close(1000);
      reject(error);
    }

    socket.addEventListener("open", () => socket.send(JSON.stringify(registration)));
    socket.addEventListener("message", (event) => {
      let message;
      try {
        message = JSON.parse(String(event.data));
      } catch {
        return;
      }
      if (message?.type === "state_snapshot" || message?.type === "state_changed") {
        const ownVolumes = message.state?.device_volumes;
        if (!ownVolumes || typeof ownVolumes !== "object" || !(clientId in ownVolumes)) return;
        projection = message;
        if (exerciseMutation && !mutationSent) {
          mutationSent = true;
          socket.send(JSON.stringify({ type: "pause" }));
          return;
        }
        if (!exerciseMutation) socket.close(1000);
      } else if (message?.type === "error" && mutationSent) {
        mutationError = message;
        socket.close(1000);
      }
    });
    socket.addEventListener("error", () => {
      if (projection && (!exerciseMutation || mutationError)) finish();
      else fail(new Error("WebSocket differential failed"));
    });
    socket.addEventListener("close", () => {
      if (projection && (!exerciseMutation || mutationError)) finish();
      else fail(new Error("WebSocket closed before the expected projection"));
    });
  });
}

async function upload(baseUrl, cookie, fixtureBytes, conflict) {
  const form = new FormData();
  form.append("files", new Blob([fixtureBytes], { type: "audio/flac" }), "differential.flac");
  return request(
    baseUrl,
    `/api/library/upload?dest=Differential&conflict=${conflict}`,
    { method: "POST", headers: { cookie }, body: form },
    { body: "json", headers: ["content-type", ...SECURITY_HEADERS] },
  );
}

async function capture(baseUrl, fixturePath, outputPath) {
  const fixtureBytes = await readFile(fixturePath);
  const observations = [];
  const record = (name, observed) => observations.push({ name, ...observed });

  let result = await request(baseUrl, "/api/health", {}, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("health", result.observed);

  result = await request(baseUrl, "/", {}, {
    body: "hash",
    headers: ["content-type", "cache-control", "etag", ...SECURITY_HEADERS],
  });
  record("spa-root", result.observed);

  result = await request(baseUrl, "/controls", {}, {
    body: "hash",
    headers: ["content-type", "cache-control", "etag", ...SECURITY_HEADERS],
  });
  record("spa-history-fallback", result.observed);

  result = await request(baseUrl, "/assets/not-present.js", {}, {
    body: "hash",
    headers: ["content-type", "cache-control", ...SECURITY_HEADERS],
  });
  record("spa-missing-asset", result.observed);

  result = await request(baseUrl, "/api/auth/me", {}, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("auth-required", result.observed);

  result = await request(baseUrl, "/api/auth/login", { method: "POST", ...jsonBody({}) }, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("login-validation", result.observed);

  result = await request(
    baseUrl,
    "/api/auth/login",
    {
      method: "POST",
      ...jsonBody({ username: "performance", password: "wrong-benchmark-password" }),
    },
    { body: "json", headers: ["content-type", ...SECURITY_HEADERS] },
  );
  record("login-invalid-credentials", result.observed);

  result = await request(
    baseUrl,
    "/api/auth/login",
    {
      method: "POST",
      ...jsonBody({ username: "performance", password: "synthetic-benchmark-password" }),
    },
    { body: "json", headers: ["content-type", "set-cookie", ...SECURITY_HEADERS] },
  );
  record("login-success", result.observed);
  const cookie = cookiePair(result.response);

  result = await request(baseUrl, "/api/auth/me", { headers: { cookie } }, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("authenticated-session", result.observed);

  result = await request(baseUrl, "/api/library/tracks/1", {}, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("guest-track", result.observed);

  result = await request(baseUrl, "/api/library/tree?path=", { headers: { cookie } }, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("authenticated-library-tree", result.observed);

  result = await request(
    baseUrl,
    "/api/library/tracks/1/stream",
    { headers: { range: "bytes=0-15" } },
    {
      body: "hash",
      headers: [
        "content-type",
        "content-length",
        "content-range",
        "accept-ranges",
        "etag",
        "last-modified",
        ...SECURITY_HEADERS,
      ],
    },
  );
  record("single-range-stream", result.observed);

  result = await request(baseUrl, "/api/library/tracks?ids=1,bad", {}, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("batch-id-validation", result.observed);

  const firstUpload = await upload(baseUrl, cookie, fixtureBytes, "rename");
  record("upload-new", firstUpload.observed);
  const uploadedId = firstUpload.observed.body?.saved?.[0]?.id;
  if (!Number.isInteger(uploadedId)) throw new Error("first upload did not return a track ID");

  const skippedUpload = await upload(baseUrl, cookie, fixtureBytes, "skip");
  record("upload-skip-conflict", skippedUpload.observed);

  const renamedUpload = await upload(baseUrl, cookie, fixtureBytes, "rename");
  record("upload-rename-conflict", renamedUpload.observed);

  result = await request(
    baseUrl,
    "/api/library/tracks/bulk-delete",
    {
      method: "POST",
      headers: { cookie, "content-type": "application/json" },
      body: JSON.stringify({ track_ids: [uploadedId, 999_999] }),
    },
    { body: "json", headers: ["content-type", ...SECURITY_HEADERS] },
  );
  record("bulk-delete-partial", result.observed);

  result = await request(baseUrl, `/api/library/tracks/${uploadedId}`, {}, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("deleted-track-missing", result.observed);

  result = await request(baseUrl, "/api/auth/logout", {
    method: "POST",
    headers: { cookie },
  }, {
    body: "json",
    headers: ["set-cookie", ...SECURITY_HEADERS],
  });
  record("logout", result.observed);

  result = await request(baseUrl, "/api/auth/me", { headers: { cookie } }, {
    body: "json",
    headers: ["content-type", ...SECURITY_HEADERS],
  });
  record("revoked-session", result.observed);

  observations.push({
    name: "guest-websocket-v2",
    status: 101,
    headers: {},
    body: await guestWebSocketProjection(baseUrl, 2, true),
  });
  await new Promise((resolve) => setTimeout(resolve, 50));
  observations.push({
    name: "guest-websocket-v1",
    status: 101,
    headers: {},
    body: await guestWebSocketProjection(baseUrl, 1, false),
  });

  const transcript = normalized({
    schema_version: "runtime-differential/v1",
    observations,
  });
  await writeFile(outputPath, `${JSON.stringify(transcript, null, 2)}\n`, "utf8");
}

function collectDifferences(left, right, path = "$", output = []) {
  if (output.length >= 50) return output;
  if (Object.is(left, right)) return output;
  if (Array.isArray(left) && Array.isArray(right)) {
    if (left.length !== right.length) output.push(`${path}.length: ${left.length} != ${right.length}`);
    const count = Math.min(left.length, right.length);
    for (let index = 0; index < count; index += 1) {
      collectDifferences(left[index], right[index], `${path}[${index}]`, output);
    }
    return output;
  }
  if (left && right && typeof left === "object" && typeof right === "object") {
    const keys = new Set([...Object.keys(left), ...Object.keys(right)]);
    for (const key of [...keys].sort()) {
      if (!(key in left)) output.push(`${path}.${key}: missing from Python`);
      else if (!(key in right)) output.push(`${path}.${key}: missing from Rust`);
      else collectDifferences(left[key], right[key], `${path}.${key}`, output);
    }
    return output;
  }
  output.push(`${path}: ${JSON.stringify(left)} != ${JSON.stringify(right)}`);
  return output;
}

async function compare(pythonPath, rustPath, reportPath) {
  const python = JSON.parse(await readFile(pythonPath, "utf8"));
  const rust = JSON.parse(await readFile(rustPath, "utf8"));
  const differences = collectDifferences(python, rust);
  let report = [
    "## Live Python/Rust semantic differential",
    "",
    `- Status: **${differences.length === 0 ? "PASS" : "FAIL"}**`,
    `- Observations: ${python.observations?.length ?? 0}`,
    "- Coverage: health, SPA fallback/cache, authentication/cookies, validation status/envelope, library reads,",
    "  single-range delivery, multipart conflict handling, partial batch failure, logout/revocation,",
    "  and guest protocol-v1/v2 WebSocket projection.",
  ];
  if (differences.length > 0) {
    report.push("", "### Differences", "", ...differences.map((difference) => `- \`${difference}\``));
  }
  report = `${report.join("\n")}\n`;
  await writeFile(reportPath, report, "utf8");
  process.stdout.write(report);
  if (differences.length > 0) {
    throw new Error("Rust runtime transcript differs from Python; see the differential report");
  }
}

const [mode, ...args] = process.argv.slice(2);
if (mode === "capture" && args.length === 3) {
  await capture(...args);
} else if (mode === "compare" && args.length === 3) {
  await compare(...args);
} else {
  throw new Error(usage());
}
