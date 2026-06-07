import type { ReactNode } from "react";

/** One label-stack form field: a small label above the control, with hint /
 *  error slots below. Wrap a single input/select/textarea. The `changed`
 *  flag paints the teal "edited / unsaved" cue. Reuses the `.field`
 *  template so forms stop re-inventing column-label layouts. */
interface FieldProps {
  label?: ReactNode;
  hint?: ReactNode;
  error?: ReactNode;
  changed?: boolean;
  className?: string;
  children: ReactNode;
}

export function Field({ label, hint, error, changed, className, children }: FieldProps) {
  const cls = ["field", changed ? "changed" : "", className].filter(Boolean).join(" ");
  return (
    <label className={cls}>
      {label !== undefined ? <span className="field-label">{label}</span> : null}
      {children}
      {error ? (
        <span className="field-hint error">{error}</span>
      ) : hint !== undefined ? (
        <span className="field-hint">{hint}</span>
      ) : null}
    </label>
  );
}
