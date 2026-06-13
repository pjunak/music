import { useEffect, useRef, useState } from "react";
import type { FormEvent } from "react";

import { Field } from "./Field";
import { useInputStore } from "./inputDialog";
import { Modal } from "./Modal";

/** Renders the open input dialog (if any). Mounts once at AppShell, like
 *  ConfirmDialogHost. The open API itself is `inputDialog()` from `./inputDialog`. */
export function InputDialogHost() {
  const current = useInputStore((s) => s.current);
  const resolve = useInputStore((s) => s.resolve);
  const [value, setValue] = useState("");
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Reset state every time a new request opens so a stale value from the
  // previous prompt can't leak in, then focus + select the field.
  useEffect(() => {
    if (current === null) return;
    setValue(current.initial ?? "");
    setError(null);
    queueMicrotask(() => {
      inputRef.current?.focus();
      inputRef.current?.select();
    });
  }, [current]);

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
    <Modal
      ariaLabel={current.title}
      title={current.title}
      className="input-dialog"
      onClose={() => resolve(null)}
      onSubmit={submit}
      footer={
        <>
          <button type="button" onClick={() => resolve(null)}>
            {current.cancelLabel ?? "Cancel"}
          </button>
          <button type="submit" className="btn-primary">
            {current.confirmLabel ?? "OK"}
          </button>
        </>
      }
    >
      {current.body !== undefined ? <p className="muted small">{current.body}</p> : null}
      <Field label={current.label} error={error ?? undefined}>
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
      </Field>
    </Modal>
  );
}
