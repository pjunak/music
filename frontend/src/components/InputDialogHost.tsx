import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";

import { useInputStore } from "./inputDialog";

/** Renders the open input dialog (if any). Mounts once at AppShell, like
 *  ConfirmDialogHost. The open API itself is `inputDialog()` from `./inputDialog`. */
export function InputDialogHost() {
  const current = useInputStore((s) => s.current);
  const resolve = useInputStore((s) => s.resolve);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Reset state every time a new request opens so a stale value from the
  // previous prompt can't leak in.
  useEffect(() => {
    if (current === null) return;
    setValue(current.initial ?? "");
    setError(null);
    // autoFocus on the <input> doesn't fire reliably when the modal is the
    // result of an async chain (we replace `current` in a then-handler) —
    // do it imperatively after the next paint.
    queueMicrotask(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [current]);

  useEffect(() => {
    if (current === null) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") resolve(null);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [current, resolve]);

  if (current === null) return null;

  const trim = current.trim ?? true;
  const required = current.required ?? true;

  function submit(e: FormEvent) {
    e.preventDefault();
    if (current === null) return;
    const final = trim ? value.trim() : value;
    if (required && final === "") {
      setError("Required.");
      return;
    }
    if (current.validate) {
      const msg = current.validate(final);
      if (msg !== null && msg !== "") {
        setError(msg);
        return;
      }
    }
    resolve(final);
  }

  return (
    <div className="modal-backdrop" onMouseDown={() => resolve(null)}>
      <form
        className="modal input-dialog"
        role="dialog"
        aria-modal="true"
        aria-label={current.title}
        onMouseDown={(e) => e.stopPropagation()}
        onSubmit={submit}
      >
        <header className="modal-header">
          <h2>{current.title}</h2>
        </header>
        <div className="modal-body">
          {current.body !== undefined ? (
            <p className="muted small">{current.body}</p>
          ) : null}
          <label className="input-dialog-field">
            {current.label !== undefined ? (
              <span className="muted small">{current.label}</span>
            ) : null}
            <input
              ref={inputRef}
              type="text"
              value={value}
              onChange={(e) => {
                setValue(e.target.value);
                if (error !== null) setError(null);
              }}
              placeholder={current.placeholder}
              pattern={current.pattern}
              title={current.patternHint}
              autoComplete="off"
              spellCheck={false}
            />
            {error !== null ? <span className="error small">{error}</span> : null}
          </label>
        </div>
        <div className="modal-actions">
          <button type="button" onClick={() => resolve(null)}>
            {current.cancelLabel ?? "Cancel"}
          </button>
          <button type="submit" className="btn-primary">
            {current.confirmLabel ?? "OK"}
          </button>
        </div>
      </form>
    </div>
  );
}
