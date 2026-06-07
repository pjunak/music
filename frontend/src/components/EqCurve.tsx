import { useCallback, useEffect, useRef } from "react";

import { computeEqResponseDb, logFreqAxis } from "@/core/eq";
import type { EqBand } from "@/core/eq";

/** Live magnitude-response curve for a graphic-EQ band set. Drawn on a canvas
 *  from `computeEqResponseDb` (the same math the engine's filters use), so it
 *  updates the instant a fader moves — no audio signal required. */

interface Props {
  bands: EqBand[];
  height?: number;
}

const DB_RANGE = 15; // vertical half-range (curve headroom beyond ±12 faders)
const GRID_HZ = [100, 1000, 10000];
const GRID_HZ_LABELS = ["100", "1k", "10k"];
const GRID_DB = [-12, -6, 0, 6, 12];

function cssVar(el: Element, name: string, fallback: string): string {
  const v = getComputedStyle(el).getPropertyValue(name).trim();
  return v || fallback;
}

export function EqCurve({ bands, height = 128 }: Props) {
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const draw = useCallback(() => {
    const wrap = wrapRef.current;
    const canvas = canvasRef.current;
    if (!wrap || !canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;

    const w = wrap.clientWidth;
    const h = height;
    if (w === 0) return;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    canvas.style.width = `${w}px`;
    canvas.style.height = `${h}px`;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, h);

    const accent = cssVar(canvas, "--accent", "#6aa6ff");
    const accentStrong = cssVar(canvas, "--accent-strong", "#9bc2ff");
    const grid = cssVar(canvas, "--border", "#2a2d33");
    const muted = cssVar(canvas, "--muted", "#8a8c91");

    const logMin = Math.log10(20);
    const logMax = Math.log10(20000);
    const xForFreq = (f: number) => ((Math.log10(f) - logMin) / (logMax - logMin)) * w;
    const yForDb = (db: number) => h * (0.5 - db / (2 * DB_RANGE));

    // Grid.
    ctx.lineWidth = 1;
    ctx.font = "10px ui-sans-serif, system-ui, sans-serif";
    ctx.textBaseline = "top";
    GRID_HZ.forEach((f, i) => {
      const x = Math.round(xForFreq(f)) + 0.5;
      ctx.strokeStyle = grid;
      ctx.beginPath();
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
      ctx.stroke();
      ctx.fillStyle = muted;
      ctx.fillText(GRID_HZ_LABELS[i], x + 3, h - 13);
    });
    GRID_DB.forEach((db) => {
      const y = Math.round(yForDb(db)) + 0.5;
      ctx.strokeStyle = grid;
      ctx.globalAlpha = db === 0 ? 1 : 0.55;
      ctx.beginPath();
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
      ctx.globalAlpha = 1;
    });

    // Response curve, sampled per pixel.
    const samples = Math.max(2, Math.floor(w));
    const freqs = logFreqAxis(samples, 20, 20000);
    const resp = computeEqResponseDb(bands, freqs);
    const pts: [number, number][] = resp.map((db, i) => [
      (i / (samples - 1)) * w,
      yForDb(db),
    ]);

    // Fill toward the 0-dB line.
    const zeroY = yForDb(0);
    ctx.beginPath();
    ctx.moveTo(pts[0][0], zeroY);
    for (const [x, y] of pts) ctx.lineTo(x, y);
    ctx.lineTo(pts[pts.length - 1][0], zeroY);
    ctx.closePath();
    const fill = ctx.createLinearGradient(0, 0, 0, h);
    fill.addColorStop(0, accent);
    fill.addColorStop(1, "transparent");
    ctx.globalAlpha = 0.16;
    ctx.fillStyle = fill;
    ctx.fill();
    ctx.globalAlpha = 1;

    // Curve stroke.
    ctx.beginPath();
    pts.forEach(([x, y], i) => (i === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y)));
    ctx.strokeStyle = accentStrong;
    ctx.lineWidth = 2;
    ctx.lineJoin = "round";
    ctx.stroke();
  }, [bands, height]);

  useEffect(() => {
    draw();
    const wrap = wrapRef.current;
    if (!wrap || typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => draw());
    ro.observe(wrap);
    return () => ro.disconnect();
  }, [draw]);

  return (
    <div className="eq-curve" ref={wrapRef} style={{ height }}>
      <canvas ref={canvasRef} aria-hidden="true" />
    </div>
  );
}
