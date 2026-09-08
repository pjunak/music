function object(value: unknown): Record<string, unknown> {
  return value && typeof value === "object" && !Array.isArray(value) ? value as Record<string, unknown> : {};
}
function number(value: unknown): string {
  return typeof value === "number" && Number.isFinite(value) ? String(Math.round(value * 100) / 100) : "unknown";
}
function label(value: unknown): string { return typeof value === "string" ? value.replaceAll("_", " ") : "unknown"; }

export function ModelInputEvidence({ input }: { input: Record<string, unknown> }) {
  const context = object(input.context_evidence);
  const trajectories = object(context.trajectories);
  const tempo = object(context.tempo);
  const voice = object(context.voice);
  const structure = object(context.structure);
  const reliability = object(context.measurement_reliability);
  const metadata = ["artist", "album", "origin", "genre"].filter((key) => typeof input[key] === "string" && input[key] !== "");
  return <details>
    <summary>Track evidence sent to the model</summary>
    <p>Saved with this result. Track identifiers and the shared vocabulary are omitted here.</p>
    {metadata.length ? <ul>{metadata.map((key) => <li key={key}>{key}: {String(input[key])}</li>)}</ul> : <p>No descriptive metadata was supplied.</p>}
    {input.context_evidence ? <>
      <p>Pulse: {number(tempo.typical_bpm)} BPM; reliability {label(reliability.tempo)}. Development: {label(structure.development)} across {number(structure.section_count)} sections.</p>
      <p>Voice: {label(voice.status)}{voice.status === "classified" ? `; score ${number(voice.voice_probability)}, coverage ${number(voice.vocal_coverage)}` : ""}. Voice presence does not identify mood or lyrics.</p>
      <ul>{["intensity", "rhythmic_drive", "density"].filter((key) => trajectories[key]).map((key) => {
        const axis = object(trajectories[key]);
        return <li key={key}>{label(key)}: {number(axis.start)} at the start → {number(axis.end)} at the end; {label(axis.shape)}, reliability {label(reliability[key])}.</li>;
      })}</ul>
      <p>These are acoustic proxies on a 0–1 scale, not emotion scores. No music mood classifier was included in this input.</p>
    </> : <p>No local audio context was supplied.</p>}
    <details><summary>Inspect the exact saved fields</summary><pre className="assistant-input-snapshot">{JSON.stringify(input, null, 2)}</pre></details>
  </details>;
}
