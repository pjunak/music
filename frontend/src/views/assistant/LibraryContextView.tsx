import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Link } from "react-router-dom";

import { FolderTree } from "@/components/FolderTree";
import { IconButton } from "@/components/IconButton";
import { LibrarySidebarRail } from "@/components/LibrarySidebarRail";
import { PauseIcon, PlayIcon, StopIcon } from "@/components/icons";
import { VolumeControl } from "@/components/VolumeControl";
import type { TrackContextDetail } from "@/core/api";
import { assistantApi, libraryApi } from "@/core/api";
import type { Track } from "@/core/types";

const TRAJECTORIES = [
  ["intensity", "Intensity"],
  ["loudness", "Signal level"],
  ["rhythmic_drive", "Rhythmic drive"],
  ["brightness", "Brightness"],
  ["density", "Spectral fullness"],
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

function timelineDuration(detail: TrackContextDetail): number {
  return Math.max(
    1,
    ...detail.timeline.map(
      (point) => (point.start_s ?? 0) + (point.duration_s ?? 0),
    ),
  );
}

function voiceLabel(
  status: string | null,
  voiceScore: number | null,
  vocalCoverage: number | null,
): string {
  if (status !== "classified" || voiceScore === null || vocalCoverage === null) {
    return status?.replaceAll("_", " ") ?? "Unknown";
  }
  if (voiceScore >= 0.65 && vocalCoverage >= 0.6) return "Voice present";
  if (voiceScore >= 0.55 && vocalCoverage >= 0.2) return "Partial voice";
  if (voiceScore <= 0.35 && vocalCoverage <= 0.2) return "Predominantly instrumental";
  return "Mixed / uncertain voice";
}

function Timeline({
  detail,
  playbackTime,
  onSeek,
}: {
  detail: TrackContextDetail;
  playbackTime: number;
  onSeek: (next: number) => void;
}) {
  if (detail.timeline.length < 2) {
    return <p className="muted">No condensed timeline is available.</p>;
  }
  const width = 720;
  const height = 180;
  const plotInsetX = 6;
  const plotInsetY = 10;
  const plotWidth = width - plotInsetX * 2;
  const plotHeight = height - plotInsetY * 2;
  const duration = timelineDuration(detail);
  const series = [
    ["intensity", "#f5a65b"],
    ["rhythmic_drive", "#57d3c8"],
    ["loudness", "#8ca8ff"],
  ] as const;
  const boundedPlaybackTime = Math.max(0, Math.min(duration, playbackTime));
  const cursorX = plotInsetX + (boundedPlaybackTime / duration) * plotWidth;
  return (
    <div className="assistant-context-chart">
      <div className="assistant-context-chart-timeline">
        <svg
          role="img"
          aria-label="Intensity, rhythmic drive, and loudness across the track"
          viewBox={`0 0 ${width} ${height}`}
          preserveAspectRatio="none"
        >
          {[0.25, 0.5, 0.75].map((fraction) => (
            <line
              key={fraction}
              x1={plotInsetX}
              x2={width - plotInsetX}
              y1={plotInsetY + plotHeight * (1 - fraction)}
              y2={plotInsetY + plotHeight * (1 - fraction)}
              className="assistant-context-grid-line"
            />
          ))}
          {series.map(([key, color]) => {
            const points = detail.timeline
              .map((point) => {
                const x = plotInsetX + ((point.start_s ?? 0) / duration) * plotWidth;
                const value = Math.max(0, Math.min(1, point[key] ?? 0));
                const y = plotInsetY + (1 - value) * plotHeight;
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
          <line
            className="assistant-context-playback-cursor"
            x1={cursorX}
            x2={cursorX}
            y1={plotInsetY}
            y2={height - plotInsetY}
            vectorEffect="non-scaling-stroke"
          />
          <circle
            className="assistant-context-playback-marker"
            cx={cursorX}
            cy={plotInsetY}
            r="5"
            vectorEffect="non-scaling-stroke"
          />
        </svg>
        <input
          className="assistant-context-timeline-scrubber"
          type="range"
          min={0}
          max={duration}
          step={0.1}
          value={boundedPlaybackTime}
          onChange={(event) => onSeek(Number(event.currentTarget.value))}
          aria-label={`Seek ${detail.title}`}
          aria-valuetext={`${seconds(boundedPlaybackTime)} of ${seconds(duration)}`}
          title="Drag to seek through the track"
        />
      </div>
      <div className="assistant-context-chart-footer">
        <div className="assistant-context-chart-legend">
          <span className="is-intensity">Intensity</span>
          <span className="is-rhythm">Rhythmic drive</span>
          <span className="is-loudness">Signal level</span>
        </div>
        <span className="assistant-context-playback-time">
          {seconds(playbackTime)} / {seconds(duration)}
        </span>
      </div>
    </div>
  );
}

function ContextDetail({ detail }: { detail: TrackContextDetail }) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const [playbackTime, setPlaybackTime] = useState(0);
  const [isPlaying, setIsPlaying] = useState(false);
  const [volume, setVolume] = useState(1);
  const [playbackError, setPlaybackError] = useState<string | null>(null);
  const summary = detail.summary;
  const trajectories = objectValue(summary?.trajectories);
  const tempo = objectValue(summary?.tempo);
  const structure = objectValue(summary?.structure);
  const voice = objectValue(summary?.voice);
  const stages = objectValue(detail.stages);
  const voiceStage = objectValue(stages?.voice);
  const voiceStatus = stringValue(voice?.status);
  // The legacy wire key is retained, but its value is a normalized classifier
  // score rather than a calibrated probability.
  const voiceScore = numberValue(voice?.voice_probability);
  const vocalCoverage = numberValue(voice?.vocal_coverage);
  const duration = timelineDuration(detail);

  useEffect(() => {
    setPlaybackTime(0);
    setIsPlaying(false);
    setPlaybackError(null);
  }, [detail.track_id]);

  useEffect(() => {
    if (audioRef.current !== null) audioRef.current.volume = volume;
  }, [detail.track_id, volume]);

  function seekTo(next: number) {
    const bounded = Math.max(0, Math.min(duration, next));
    if (audioRef.current !== null) audioRef.current.currentTime = bounded;
    setPlaybackTime(bounded);
  }

  function togglePlayback() {
    const audio = audioRef.current;
    if (audio === null) return;
    setPlaybackError(null);
    if (isPlaying) {
      audio.pause();
      return;
    }
    if (audio.ended || audio.currentTime >= duration) seekTo(0);
    void audio.play().catch(() => {
      setIsPlaying(false);
      setPlaybackError("Playback could not be started. Try again or check the audio file.");
    });
  }

  function stopPlayback() {
    const audio = audioRef.current;
    if (audio !== null) {
      audio.pause();
      audio.currentTime = 0;
    }
    setIsPlaying(false);
    setPlaybackTime(0);
    setPlaybackError(null);
  }

  function changeVolume(next: number) {
    setVolume(next);
    if (audioRef.current !== null) audioRef.current.volume = next;
  }

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
        <Link to="/assistant/moods/workflow">Open context analysis</Link>
      </div>
    );
  }

  return (
    <div className="assistant-context-detail">
      <section className="assistant-context-development">
        <div className="assistant-context-detail-heading">
          <div className="assistant-context-title">
            <h2>{detail.title}</h2>
            <span>{detail.artist || "Unknown artist"}</span>
          </div>
          <span
            className={`assistant-job-status assistant-context-status is-${detail.status === "full" ? "succeeded" : "queued"}`}
          >
            {detail.status} · {detail.confidence ?? "unknown"} confidence
          </span>
        </div>
        <audio
          ref={audioRef}
          className="assistant-context-audio"
          aria-label={`Audio preview for ${detail.title}`}
          preload="none"
          src={libraryApi.streamUrl(detail.track_id)}
          onPlay={() => setIsPlaying(true)}
          onPause={() => setIsPlaying(false)}
          onEnded={() => setIsPlaying(false)}
          onTimeUpdate={(event) => setPlaybackTime(event.currentTarget.currentTime)}
          onSeeked={(event) => setPlaybackTime(event.currentTarget.currentTime)}
        >
          <track kind="captions" />
        </audio>
        <Timeline detail={detail} playbackTime={playbackTime} onSeek={seekTo} />
        <div
          className="assistant-context-preview-controls"
          role="group"
          aria-label={`${detail.title} preview controls`}
        >
          <div className="assistant-context-preview-transport">
            <IconButton
              label={`${isPlaying ? "Pause" : "Play"} ${detail.title}`}
              icon={isPlaying ? <PauseIcon /> : <PlayIcon />}
              onClick={togglePlayback}
              variant="primary"
            >
              {isPlaying ? "Pause" : "Play"}
            </IconButton>
            <IconButton
              label={`Stop ${detail.title}`}
              icon={<StopIcon />}
              onClick={stopPlayback}
              disabled={!isPlaying && playbackTime <= 0}
            >
              Stop
            </IconButton>
          </div>
          <VolumeControl
            className="assistant-context-preview-volume"
            value={volume}
            onChange={changeVolume}
            label={`${detail.title} preview volume`}
          />
        </div>
        {playbackError !== null ? (
          <p className="error assistant-context-playback-error" role="alert">{playbackError}</p>
        ) : null}
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
          <strong>
            {voiceStatus === "unavailable" && stringValue(voiceStage?.reason) === "model_missing"
              ? "Model file missing"
              : voiceLabel(voiceStatus, voiceScore, vocalCoverage)}
          </strong>
          <small>
            {voiceStatus === "classified" && voiceScore !== null && vocalCoverage !== null
              ? `${percent(voiceScore)} voice score · ${percent(vocalCoverage)} vocal coverage`
              : (stringValue(voice?.note) ?? "No voice classifier result")}
          </small>
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
                brightness {percent(section.brightness)} · fullness {percent(section.density)}
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
      <h1 className="sr-only">Track context</h1>
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
