import type { AudioSignalProfile } from "@/core/api";

function metric(profile: AudioSignalProfile, key: string): number | null {
  const value = profile.metrics[key];
  return typeof value === "number" ? value : null;
}

interface AudioSignalEvidenceProps {
  profile: AudioSignalProfile | null;
}

export function AudioSignalEvidence({ profile }: AudioSignalEvidenceProps) {
  if (profile === null) {
    return (
      <div className="assistant-tag-source is-signal">
        <div>
          <strong>Audio signal evidence</strong>
          <span>
            No current signal profile. Run Audio signal profiles above to measure
            this track.
          </span>
        </div>
      </div>
    );
  }

  const rms = metric(profile, "rms_dbfs");
  const spread = metric(profile, "level_spread_db");
  const brightness = metric(profile, "high_frequency_ratio");
  const tempo = metric(profile, "tempo_bpm");

  return (
    <div className="assistant-tag-source is-signal">
      <div className="assistant-signal-heading">
        <div>
          <strong>Audio signal evidence</strong>
          <span>
            Read-only measurements from {profile.analyzer_id}; never promoted to
            manual tags automatically.
          </span>
        </div>
        <span className={`assistant-confidence is-${profile.confidence}`}>
          {profile.confidence}
        </span>
      </div>
      <div className="assistant-signal-metrics">
        <div>
          <strong>{rms === null ? "—" : `${rms.toFixed(1)} dBFS`}</strong>
          <span>Average level</span>
        </div>
        <div>
          <strong>{spread === null ? "—" : `${spread.toFixed(1)} dB`}</strong>
          <span>Level spread</span>
        </div>
        <div>
          <strong>{brightness === null ? "—" : brightness.toFixed(3)}</strong>
          <span>Brightness proxy</span>
        </div>
        <div>
          <strong>{tempo === null ? "No stable pulse" : `${tempo.toFixed(1)} BPM`}</strong>
          <span>Tempo estimate</span>
        </div>
      </div>
      <details className="assistant-signal-evidence">
        <summary>How these measurements were interpreted</summary>
        <ul>
          {profile.evidence.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      </details>
    </div>
  );
}
