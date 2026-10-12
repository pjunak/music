
export const BASE = import.meta.env.VITE_API_BASE_URL ?? "";

export class ApiError extends Error {
  constructor(
    public status: number,
    public detail: string,
  ) {
    super(`API ${status}: ${detail}`);
  }
}

/** Called on every 401 response, before the ApiError is thrown to the
 *  caller. Registered by the auth store (a direct import here would be a
 *  cycle — auth already imports this module). The handler decides whether
 *  the 401 means "session lost mid-use" (operator was authenticated) or
 *  nothing (boot-time /me probe, wrong password on login). */
let onUnauthorized: (() => void) | null = null;

export function setUnauthorizedHandler(fn: () => void): void {
  onUnauthorized = fn;
}

async function request<T>(method: string, path: string, body?: unknown): Promise<T> {
  const init: RequestInit = { method, credentials: "include" };
  if (body instanceof FormData) {
    init.body = body;
  } else if (body !== undefined) {
    init.headers = { "Content-Type": "application/json" };
    init.body = JSON.stringify(body);
  }
  let response: Response;
  try {
    response = await fetch(`${BASE}${path}`, init);
  } catch (err) {
    // fetch() rejecting (vs an HTTP error) is a network-level failure — the
    // classic case is the first request after the device wakes from
    // minimize, while Wi-Fi/DNS are still coming back. GETs are idempotent,
    // so retry once after a beat instead of surfacing a warning for a blip
    // that self-heals a second later. Mutations still fail fast.
    if (method !== "GET") throw err;
    await new Promise((r) => setTimeout(r, 1000));
    response = await fetch(`${BASE}${path}`, init);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  const contentType = response.headers.get("content-type") ?? "";
  if (!response.ok) {
    throw responseError(response.status, response.statusText, contentType, text);
  }
  if (!text) return undefined as T;
  if (!contentType.includes("application/json")) {
    throw responseError(response.status, response.statusText, contentType, text);
  }
  try {
    return JSON.parse(text) as T;
  } catch {
    // A success status with an unreadable body must not reach callers as undefined.
    throw new ApiError(response.status, "malformed JSON response");
  }
}

/** Build the ApiError for a failed or non-JSON response, notifying the auth
 *  store on 401. Shared by fetch, XHR uploads and file downloads. */
function responseError(
  status: number,
  statusText: string,
  contentType: string,
  text: string,
): ApiError {
  const looksLikeJson = contentType.includes("application/json");
  let fromJson: string | null = null;
  if (text && looksLikeJson) {
    try {
      const parsed: unknown = JSON.parse(text);
      if (parsed && typeof parsed === "object" && "detail" in parsed) {
        fromJson = formatApiDetail(parsed.detail);
      }
    } catch {
      // fall through to the status text.
    }
  }
  const snippet = text.slice(0, 200).replace(/\s+/g, " ").trim();
  const detail =
    fromJson ??
    (looksLikeJson || !text
      ? statusText || `HTTP ${status}`
      : `non-JSON response (${contentType || "no content-type"}): ${snippet}`);
  if (status === 401) onUnauthorized?.();
  return new ApiError(status, detail);
}

/** Download a binary API response (e.g. a backup archive) as a Blob. */
export async function downloadBlob(path: string): Promise<{ blob: Blob; filename: string | null }> {
  const response = await fetch(`${BASE}${path}`, { credentials: "include" });
  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw responseError(
      response.status,
      response.statusText,
      response.headers.get("content-type") ?? "",
      text,
    );
  }
  const disposition = response.headers.get("content-disposition") ?? "";
  const filename = disposition.match(/filename="([^"]+)"/)?.[1] ?? null;
  return { blob: await response.blob(), filename };
}

export const api = {
  get: <T>(path: string) => request<T>("GET", path),
  post: <T>(path: string, body?: unknown) => request<T>("POST", path, body),
  put: <T>(path: string, body?: unknown) => request<T>("PUT", path, body),
  patch: <T>(path: string, body?: unknown) => request<T>("PATCH", path, body),
  delete: <T>(path: string) => request<T>("DELETE", path),
};

// XHR (not fetch) because only XHR exposes upload-progress events.
export function uploadWithProgress<T>(
  path: string,
  files: File[],
  onProgress?: (loaded: number, total: number) => void,
): Promise<T> {
  const form = new FormData();
  for (const f of files) form.append("files", f, f.name);
  return new Promise<T>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open("POST", `${BASE}${path}`);
    xhr.withCredentials = true;
    xhr.responseType = "text";
    xhr.upload.onprogress = (e) => {
      if (onProgress && e.lengthComputable) onProgress(e.loaded, e.total);
    };
    xhr.onerror = () => reject(new Error("network error during upload"));
    xhr.onload = () => {
      if (xhr.status >= 200 && xhr.status < 300) {
        try {
          resolve(JSON.parse(xhr.responseText) as T);
        } catch {
          reject(new Error("upload succeeded but response wasn't JSON"));
        }
        return;
      }
      reject(
        responseError(
          xhr.status,
          xhr.statusText,
          xhr.getResponseHeader("content-type") ?? "",
          xhr.responseText,
        ),
      );
    };
    xhr.send(form);
  });
}

// --- typed helpers per resource -----------------------------------------

function formatApiDetail(detail: unknown): string | null {
  if (typeof detail === "string") return detail;
  if (Array.isArray(detail)) {
    const messages = detail.flatMap((entry) => {
      if (!entry || typeof entry !== "object") return [];
      const candidate = entry as { loc?: unknown; msg?: unknown };
      if (typeof candidate.msg !== "string") return [];
      const location = Array.isArray(candidate.loc)
        ? candidate.loc.filter((part) => part !== "body").join(".")
        : "";
      return [`${location ? `${location}: ` : ""}${candidate.msg}`];
    });
    if (messages.length > 0) return messages.slice(0, 4).join(" · ");
  }
  if (detail && typeof detail === "object") {
    const providerDetail = detail as { message?: unknown };
    if (typeof providerDetail.message === "string") return providerDetail.message;
    try {
      return JSON.stringify(detail);
    } catch {
      return null;
    }
  }
  return typeof detail === "number" || typeof detail === "boolean" || typeof detail === "bigint"
    ? String(detail)
    : null;
}
