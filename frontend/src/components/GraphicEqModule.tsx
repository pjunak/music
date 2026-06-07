import {
  EQ_BAND_LABELS,
  EQ_GAIN_MAX,
  EQ_GAIN_MIN,
  EQ_GAIN_STEP,
  defaultEqBands,
  isFlat,
} from "@/core/eq";
import type { EqBand } from "@/core/eq";

import { EqCurve } from "./EqCurve";
import { Fader } from "./Fader";

/** The graphic-EQ editor: a live response curve above a row of vertical band
 *  faders (one octave-band each). Double-click a fader or hit "Flat" to reset
 *  to 0 dB. Pure presentation — the parent owns the band array. */

interface Props {
  bands: EqBand[];
  onChange: (bands: EqBand[]) => void;
}

const fmtDb = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(1)}`;

export function GraphicEqModule({ bands, onChange }: Props) {
  function setBand(i: number, gain: number) {
    onChange(bands.map((b, j) => (j === i ? { ...b, gain } : b)));
  }

  return (
    <div className="graphic-eq">
      <EqCurve bands={bands} />
      <div className="eq-bands">
        {bands.map((b, i) => (
          <Fader
            key={b.frequency}
            bipolar
            value={b.gain}
            min={EQ_GAIN_MIN}
            max={EQ_GAIN_MAX}
            step={EQ_GAIN_STEP}
            height={120}
            label={EQ_BAND_LABELS[i]}
            format={fmtDb}
            def={0}
            ariaLabel={`${EQ_BAND_LABELS[i]} Hz band gain`}
            onChange={(v) => setBand(i, v)}
          />
        ))}
      </div>
      <div className="eq-module-footer">
        <span className="muted small">dB · drag a band, double-click to zero</span>
        <button
          type="button"
          className="btn-ghost"
          onClick={() => onChange(defaultEqBands())}
          disabled={isFlat(bands)}
        >
          Flat
        </button>
      </div>
    </div>
  );
}
