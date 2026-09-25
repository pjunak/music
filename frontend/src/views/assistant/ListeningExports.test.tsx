import { execFile } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./downloadJson", () => ({ downloadJson: vi.fn() }));
vi.mock("@/core/toast", () => ({ toast: { error: vi.fn() } }));

import { toast } from "@/core/toast";
import { downloadJson } from "./downloadJson";
import { ListeningSampleExport } from "./ListeningSampleExport";
import { TaggingRunExport } from "./TaggingRunExport";

beforeEach(() => { vi.clearAllMocks(); });

describe("listening sample export", () => {
  it("exports a sorted prediction-free inventory without changing the selection", async () => {
    const ids = [8, 2, 5], user = userEvent.setup();
    render(<ListeningSampleExport trackIds={ids} />);
    await user.click(screen.getByText("Listening comparison"));
    await user.click(screen.getByRole("button", { name: "Export listening sample" }));
    expect(downloadJson).toHaveBeenCalledWith({
      schema_version: "song-mood-inventory/v1", track_ids: [2, 5, 8],
    }, "mood-listening-inventory.json");
    expect(ids).toEqual([8, 2, 5]);
  });

  it.each([[1], [1, 1], [0, 2], [1.5, 2], Array.from({ length: 1001 }, (_, index) => index + 1)])(
    "keeps an invalid or oversized sample unavailable (%#)", async (...ids) => {
      const user = userEvent.setup();
      render(<ListeningSampleExport trackIds={ids} />);
      await user.click(screen.getByText("Listening comparison"));
      const button = screen.getByRole("button", { name: "Export listening sample" });
      expect(button).toBeDisabled();
      await user.click(button);
      expect(downloadJson).not.toHaveBeenCalled();
    },
  );

  it("reports a failed download without changing any selection", async () => {
    vi.mocked(downloadJson).mockImplementationOnce(() => { throw new Error("download unavailable"); });
    const user = userEvent.setup();
    render(<ListeningSampleExport trackIds={[2, 5]} />);
    await user.click(screen.getByText("Listening comparison"));
    await user.click(screen.getByRole("button", { name: "Export listening sample" }));
    expect(toast.error).toHaveBeenCalledWith("Could not export the listening sample");
  });
});

describe("retained run export", () => {
  it.each([null, { track_results: [] }, { feature_progress: { track_results: [] } }])(
    "exports zero retained results as missing, never as invented empty per-track answers (%#)", async (result) => {
      const user = userEvent.setup();
      render(<TaggingRunExport id="failed-run" result={result} />);
      await user.click(screen.getByRole("button", { name: "Export retained run results" }));
      expect(downloadJson).toHaveBeenCalledWith({
        schema_version: "assistant-mood-run-export/v1",
        run_id: "failed-run", track_results: [], usage: null,
      }, "mood-run-failed-run.json");
    },
  );

  it("retains checkpoint answers and usage for a partially failed run", async () => {
    const rows = [{ track_id: 2, tags: [], source_signature: "current-source" }];
    const user = userEvent.setup();
    render(<TaggingRunExport id="partial-run" result={{
      feature_progress: { track_results: rows }, usage: { total_tokens: 80 },
    }} />);
    await user.click(screen.getByRole("button", { name: "Export retained run results" }));
    expect(downloadJson).toHaveBeenCalledWith({
      schema_version: "assistant-mood-run-export/v1",
      run_id: "partial-run", track_results: rows, usage: { total_tokens: 80 },
    }, "mood-run-partial-run.json");
  });

  it("keeps an explicit final empty result authoritative over checkpoint data", async () => {
    const user = userEvent.setup();
    render(<TaggingRunExport id="empty-final" result={{
      track_results: [], feature_progress: { track_results: [{ track_id: 2, tags: ["calm"] }] },
    }} />);
    await user.click(screen.getByRole("button", { name: "Export retained run results" }));
    expect(downloadJson).toHaveBeenCalledWith(expect.objectContaining({ track_results: [] }), "mood-run-empty-final.json");
  });

  it("does not manufacture missing-result reports from malformed retained rows", () => {
    render(<TaggingRunExport id="malformed" result={{ track_results: "invalid" }} />);
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(downloadJson).not.toHaveBeenCalled();
  });
});

it("consumes real UI exports in the pilot CLI without dropping a failed cohort", async () => {
  const directory = await mkdtemp(join(tmpdir(), "music-listening-export-"));
  try {
    const user = userEvent.setup();
    const view = render(<ListeningSampleExport trackIds={[5, 2]} />);
    await user.click(screen.getByText("Listening comparison"));
    await user.click(screen.getByRole("button", { name: "Export listening sample" }));
    const inventory = vi.mocked(downloadJson).mock.calls[0][0];
    const inventoryPath = join(directory, "inventory.json"), vocabularyPath = join(directory, "vocabulary.json");
    const draftPath = join(directory, "draft.jsonl"), pilotPath = join(directory, "pilot.jsonl"), runPath = join(directory, "run.json");
    await writeFile(inventoryPath, JSON.stringify(inventory));
    await writeFile(vocabularyPath, JSON.stringify({ groups: [{ key: "mood", tags: [{ id: "mood.calm", name: "calm" }] }] }));
    const command = promisify(execFile);
    const cli = resolve(dirname(fileURLToPath(import.meta.url)), "../../../../tools/mood-pilot.mjs");
    await command(process.execPath, [cli, "init", inventoryPath, vocabularyPath, draftPath]);
    const rows = (await readFile(draftPath, "utf8")).trim().split("\n").map((line) => JSON.parse(line) as Record<string, unknown>);
    expect(rows.slice(1).map((row) => row.track_id)).toEqual([2, 5]);
    expect(rows.slice(1).every((row) => row.reviewed === false && row.blind === null)).toBe(true);
    // Only this synthetic regression fixture supplies judgments. Initialization does not.
    Object.assign(rows[0], { annotator: "test", core_tag_ids: ["mood.calm"] });
    rows.slice(1).forEach((row, i) => Object.assign(row, { file_reference: "fixture-" + i, recording_group: "group-" + i,
      reviewed: true, blind: true, listened_intervals: [[0, 1]], labels: { "mood.calm": "positive" } }));
    await writeFile(draftPath, rows.map((row) => JSON.stringify(row)).join("\n") + "\n");
    await command(process.execPath, [cli, "freeze", draftPath, vocabularyPath, pilotPath]);
    view.rerender(<TaggingRunExport id="no-retained-results" result={null} />);
    await user.click(screen.getByRole("button", { name: "Export retained run results" }));
    await writeFile(runPath, JSON.stringify(vi.mocked(downloadJson).mock.calls[1][0]));
    const { stdout } = await command(process.execPath, [cli, "score", pilotPath, runPath, vocabularyPath]);
    const score = JSON.parse(stdout);
    expect(score.categories.all.tracks).toBe(1);
    expect(score.categories.all.missing_results).toBe(1);
    expect(score.categories.all.empty_results).toBe(0);
    expect(score.categories.all.precision).toBeNull();
    expect(score.categories.all.recall).toBe(0);
  } finally { await rm(directory, { recursive: true, force: true }); }
}, 10_000);
