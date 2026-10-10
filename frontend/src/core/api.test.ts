import { afterEach, describe, expect, it, vi } from "vitest";

import { ApiError, api, downloadBlob, setUnauthorizedHandler } from "@/core/api";

function respond(body: string, init: ResponseInit & { contentType?: string } = {}) {
  const headers = new Headers(init.headers);
  if (init.contentType) headers.set("content-type", init.contentType);
  return new Response(body, { ...init, headers });
}

afterEach(() => {
  vi.unstubAllGlobals();
  setUnauthorizedHandler(() => {});
});

describe("API responses", () => {
  it("rejects a success status whose JSON body is unreadable", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => respond("{not json", { status: 200, contentType: "application/json" })),
    );
    await expect(api.get("/api/thing")).rejects.toEqual(
      new ApiError(200, "malformed JSON response"),
    );
  });

  it("formats validation details and notifies the auth store on 401", async () => {
    const unauthorized = vi.fn();
    setUnauthorizedHandler(unauthorized);
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        respond(JSON.stringify({ detail: [{ loc: ["body", "name"], msg: "required" }] }), {
          status: 401,
          contentType: "application/json",
        }),
      ),
    );
    await expect(api.post("/api/thing", {})).rejects.toEqual(
      new ApiError(401, "name: required"),
    );
    expect(unauthorized).toHaveBeenCalledOnce();
  });

  it("downloads a file with its server filename and shares 401 handling", async () => {
    const unauthorized = vi.fn();
    setUnauthorizedHandler(unauthorized);
    vi.stubGlobal(
      "fetch",
      vi
        .fn()
        .mockResolvedValueOnce(
          respond("archive", {
            status: 200,
            headers: { "content-disposition": 'attachment; filename="music-backup-1.tar.gz"' },
          }),
        )
        .mockResolvedValueOnce(
          respond(JSON.stringify({ detail: "Not authenticated" }), {
            status: 401,
            contentType: "application/json",
          }),
        ),
    );
    const { blob, filename } = await downloadBlob("/api/admin/backup");
    expect(filename).toBe("music-backup-1.tar.gz");
    expect(await blob.text()).toBe("archive");
    await expect(downloadBlob("/api/admin/backup")).rejects.toEqual(
      new ApiError(401, "Not authenticated"),
    );
    expect(unauthorized).toHaveBeenCalledOnce();
  });
});
