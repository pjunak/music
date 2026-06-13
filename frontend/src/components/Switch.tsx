import type { InputHTMLAttributes, ReactNode } from "react";

/** Styled boolean toggle for consequential on/off settings (a hidden native
 *  checkbox + a CSS track/thumb, teal on-state, ≥44px hit area on coarse
 *  pointers). Use instead of a raw <input type="checkbox"> when the toggle
 *  carries weight (audio output, blackout). */
interface SwitchProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "type"> {
  /** Optional visible text after the track. */
  label?: ReactNode;
}

export function Switch({ label, className, ...rest }: SwitchProps) {
  return (
    <label className={["switch", className].filter(Boolean).join(" ")}>
      <input type="checkbox" {...rest} />
      <span className="switch-track" aria-hidden="true" />
      {label !== undefined ? <span>{label}</span> : null}
    </label>
  );
}
