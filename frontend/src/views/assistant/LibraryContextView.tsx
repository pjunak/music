import { useCallback, useEffect, useMemo, useState } from "react";
import { Link } from "react-router-dom";

import { FolderTree } from "@/components/FolderTree";
import { LibrarySidebarRail } from "@/components/LibrarySidebarRail";
import type { TrackContextDetail } from "@/core/api";
import { assistantApi, libraryApi } from "@/core/api";
import type { Track } from "@/core/types";

import { AssistantInfoPopover } from "./AssistantInfoPopover";

const TRAJECTORIES = [
  ["intensity", "Intensity"],
  ["loudness", "Loudness"],
  ["rhythmic_drive", "Rhythmic drive"],
  ["brightness", "Brightness"],
  ["density", "Density"],
  ["spectral_flux", "Spectral change"],
] as const;

function numberValue(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" && value.trim() !== "" ? value : null;
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function percent(value: unknown): string {
  const number = numberValue(value);
  return number === null ? "—" : `${Math.round(number * 100)}%`;
}

function seconds(value: unknown): string {
  const number = numberValue(value);
  if (number === null) return "—";
  const minutes = Math.floor(number / 60);
  return `${minutes}:${String(Math.round(number % 60)).padStart(2, "0")}`;
}

function Timeline({ detail }: { detail: TrackContextDetail }) {
  if (detail.timeline.length < 2) {
    return <p className="muted">No condensed timeline is available.</p>;
  }
  const width = 720;
  const height = 180;
  const duration = Math.max(
    1,
    ...detail.timeline.map(
      (point) => (point.start_s ?? 0) + (point.duration_s ?? 0),
    ),
  );
  const series = [
    ["intensity", "#f5a65b"],
    ["rhythmic_drive", "#57d3c8"],
    ["loudness", "#8ca8ff"],
  ] as const;
  return (
    <div className="assistant-context-chart">
      <svg
        role="img"
        aria-label="Intensity, rhythmic drive, and loudness across the track"
        viewBox={`0 0 ${width} ${height}`}
      >
        {[0.25, 0.5, 0.75].map((fraction) => (
          <line
            key={fraction}
            x1="0"
            x2={width}
            y1={height * (1 - fraction)}
            y2={height * (1 - fraction)}
            className="assistant-context-grid-line"
          />
        ))}
        {series.map(([key, color]) => {
          const points = detail.timeline
            .map((point) => {
              const x = ((point.start_s ?? 0) / duration) * width;
              const y = height - Math.max(0, Math.min(1, point[key] ?? 0)) * height;
              return `${x.toFixed(2)},${y.toFixed(2)}`;
            })
            .join(" ");
          return (
            <polyline
              key={key}
              points={points}
              fill="none"
              stroke={color}
              strokeWidth="3"
              vectorEffect="non-scaling-stroke"
            />
          );
        })}
      </svg>
      <div className="assistant-context-chart-legend">
        <span className="is-intensity">Intensity</span>
        <span className="is-rhythm">Rhythmic drive</span>
        <span className="is-loudness">Loudness</span>
      </div>
    </div>
  );
}

function ContextDetail({ detail }: { detail: TrackContextDetail }) {
  const summary = detail.summary;
  const trajectories = objectValue(summary?.trajectories);
  const tempo = objectValue(summary?.tempo);
  const structure = objectValue(summary?.structure);
  const voice = objectValue(summary?.voice);

  if (summary === null) {
    return (
      <div className="assistant-context-empty-detail">
        <span className={`assistant-job-status is-${detail.status === "failed" ? "failed" : "queued"}`}>
          {detail.status}
        </span>
        <h2>{detail.title}</h2>
        <p>
          {detail.error ??
            (detail.status === "stale"
              ? "The audio file changed after this context was built. Analyze it again."
              : "No current analysis context is available for this track.")}
        </p>
        <Link to="/assistant/analysis">Open context analysis</Link>
      </div>
    );
  }

  return (
    <div className="assistant-context-detail">
      <div className="assistant-context-detail-heading">
        <div>
          <span className={`assistant-job-status is-${detail.status === "full" ? "succeeded" : "queued"}`}>
            {detail.status} · {detail.confidence ?? "unknown"} confidence
          </span>
          <h2>{detail.title}</h2>
          <p>{detail.artist || "Unknown artist"}</p>
        </div>
        <audio
          aria-label={`Play ${detail.title}`}
          controls
          preload="none"
          src={libraryApi.streamUrl(detail.track_id)}
        >
          <track kind="captions" />
        </audio>
      </div>

      <section>
        <h3>Development across the track</h3>
        <Timeline detail={detail} />
        <div className="assistant-context-trajectories">
          {TRAJECTORIES.map(([key, label]) => {
            const trajectory = objectValue(trajectories?.[key]);
            return (
              <div key={key}>
                <span>{label}</span>
                <strong>{percent(trajectory?.typical)}</strong>
                <small>
                  {(stringValue(trajectory?.shape) ?? "unknown shape").replaceAll("_", " ")} ·{" "}
                  {percent(trajectory?.low)}–{percent(trajectory?.high)}
                </small>
              </div>
            );
          })}
        </div>
      </section>

      <section className="assistant-context-facts">
        <div>
          <span>Tempo</span>
          <strong>
            {tempo?.status === "measured"
              ? `${numberValue(tempo.typical_bpm)?.toFixed(1) ?? "—"} BPM`
              : "Unresolved"}
          </strong>
          <small>
            {tempo?.status === "measured"
              ? `${numberValue(tempo.low_bpm)?.toFixed(1) ?? "—"}–${numberValue(tempo.high_bpm)?.toFixed(1) ?? "—"} BPM`
              : "No sufficiently stable pulse"}
          </small>
        </div>
        <div>
          <span>Structure</span>
          <strong>{stringValue(structure?.development) ?? "Unknown"}</strong>
          <small>{numberValue(structure?.section_count) ?? 0} acoustic sections</small>
        </div>
        <div>
          <span>Voice</span>
          <strong>{stringValue(voice?.status)?.replaceAll("_", " ") ?? "Unknown"}</strong>
          <small>{stringValue(voice?.note) ?? "No voice classifier result"}</small>
        </div>
      </section>

      <section>
        <h3>Major acoustic sections</h3>
        <div className="assistant-context-sections">
          {detail.sections.map((section, index) => (
            <article key={stringValue(section.id) ?? index}>
              <div>
                <strong>{stringValue(section.id) ?? `Section ${index + 1}`}</strong>
                <span>{seconds(section.start_s)}–{seconds(section.end_s)}</span>
              </div>
              <p>
                Intensity {percent(section.intensity)} · rhythm {percent(section.rhythmic_drive)} ·
                brightness {percent(section.brightness)} · density {percent(section.density)}
              </p>
              {Array.isArray(section.changes_from_previous) && section.changes_from_previous.length > 0 ? (
                <small>{section.changes_from_previous.join(" · ").replaceAll("_", " ")}</small>
              ) : null}
            </article>
          ))}
        </div>
      </section>

      <details className="assistant-context-technical">
        <summary>Technical and analysis-stage details</summary>
        <pre>{JSON.stringify({ technical: detail.technical, stages: detail.stages }, null, 2)}</pre>
      </details>
    </div>
  );
}

export function LibraryContextView() {
  const [path, setPath] = useState("");
  const [tracks, setTracks] = useState<Track[]>([]);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [detail, setDetail] = useState<TrackContextDetail | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const [detailError, setDetailError] = useState<string | null>(null);

  const loadFolders = useCallback(async () => {
    const response = await libraryApi.allFolders();
    return response.folders.map((folder) => ({
      ...folder,
      badge: folder.track_count > 0 ? String(folder.track_count) : null,
    }));
  }, []);

  useEffect(() => {
    let disposed = false;
    setListError(null);
    void libraryApi
      .tree(path)
      .then((response) => {
        if (disposed) return;
        setTracks(response.tracks);
        setSelectedId((current) =>
          response.tracks.some((track) => track.id === current)
            ? current
            : (response.tracks[0]?.id ?? null),
        );
      })
      .catch((error: unknown) => {
        if (disposed) return;
        setTracks([]);
        setSelectedId(null);
        setListError(error instanceof Error ? error.message : "Tracks could not be loaded.");
      });
    return () => {
      disposed = true;
    };
  }, [path]);

  useEffect(() => {
    if (selectedId === null) {
      setDetail(null);
      return;
    }
    let disposed = false;
    setDetail(null);
    setDetailError(null);
    void assistantApi
      .getTrackContext(selectedId)
      .then((next) => {
        if (!disposed) setDetail(next);
      })
      .catch((error: unknown) => {
        if (!disposed) {
          setDetailError(
            error instanceof Error ? error.message : "Track context could not be loaded.",
          );
        }
      });
    return () => {
      disposed = true;
    };
  }, [selectedId]);

  const selectedTrack = useMemo(
    () => tracks.find((track) => track.id === selectedId) ?? null,
    [selectedId, tracks],
  );

  return (
    <div className="library-view assistant-context-view">
      <header className="library-toolbar assistant-context-toolbar">
        <div className="assistant-context-toolbar-title">
          <h1>Track context</h1>
          <span>Local evidence used by mood tagging</span>
        </div>
        <AssistantInfoPopover label="About this data" title="Factual, local evidence">
          <p>
            This view mirrors the Library. The selected folder already supplies the
            relative path, while the inspector shows condensed whole-track dynamics,
            tempo, structure, and analysis confidence.
          </p>
          <p>The local analyzer never proposes terrain, scene, or mood tags.</p>
        </AssistantInfoPopover>
        <Link className="btn-secondary assistant-context-refresh" to="/assistant/analysis">
          Build or refresh
        </Link>
      </header>

      <div className="music-workspace assistant-context-workspace">
        <LibrarySidebarRail>
          <FolderTree selectedPath={path} onSelect={setPath} loadAll={loadFolders} />
        </LibrarySidebarRail>
        <section className="library-main assistant-context-tracks" aria-label="Tracks in selected folder">
          <div className="folder-header assistant-context-folder-header">
            <button type="button" className="btn-ghost" onClick={() => setPath("")}>Music</button>
            <span>{path || "Library root"}</span>
            <small>{tracks.length} track{tracks.length === 1 ? "" : "s"}</small>
          </div>
          {listError !== null ? <p className="error">{listError}</p> : null}
          {tracks.length === 0 && listError === null ? (
            <p className="muted">No tracks directly in this folder.</p>
          ) : (
            <div className="track-table-wrap assistant-context-track-table-wrap">
              <table className="track-table assistant-context-track-table">
                <thead>
                  <tr><th>Name</th><th>Artist</th></tr>
                </thead>
                <tbody>
                  {tracks.map((track) => (
                    <tr
                      key={track.id}
                      className={`track-row${track.id === selectedId ? " focused" : ""}`}
                      aria-selected={track.id === selectedId}
                      tabIndex={0}
                      onClick={() => setSelectedId(track.id)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.key === " ") {
                          event.preventDefault();
                          setSelectedId(track.id);
                        }
                      }}
                    >
                      <td><strong>{track.display_title || track.title}</strong></td>
                      <td>{track.artist || "Unknown artist"}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
        <aside className="library-inspector assistant-context-main">
          <div className="tag-inspector assistant-context-inspector">
            {detailError !== null ? (
              <p className="error">{detailError}</p>
            ) : selectedId !== null && detail === null ? (
              <p className="muted">Loading {selectedTrack?.display_title || selectedTrack?.title || "track"}…</p>
            ) : detail !== null ? (
              <ContextDetail detail={detail} />
            ) : (
              <p className="muted">Select a track to inspect its context.</p>
            )}
          </div>
        </aside>
      </div>
    </div>
  );
}
