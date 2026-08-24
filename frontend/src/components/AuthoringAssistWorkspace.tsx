import type { ReactNode } from "react";

import { SparkleIcon } from "./icons";

interface Props {
  title: string;
  description: string;
  onClose: () => void;
  children: ReactNode;
}

/**
 * Hosts an existing review-first Assistant workflow inside Authoring. The
 * server still owns validation and persistence; this frame only keeps the
 * drafting and subsequent manual editing in one visible workspace.
 */
export function AuthoringAssistWorkspace({
  title,
  description,
  onClose,
  children,
}: Props) {
  return (
    <section className="authoring-assist-workspace" aria-labelledby="authoring-assist-title">
      <header className="authoring-assist-heading">
        <div className="authoring-assist-mark" aria-hidden="true">
          <SparkleIcon />
        </div>
        <div>
          <p className="assistant-eyebrow">Optional drafting sidecar</p>
          <h2 id="authoring-assist-title">{title}</h2>
          <p>{description}</p>
        </div>
        <button type="button" className="btn-ghost" onClick={onClose}>
          Close assistant
        </button>
      </header>
      <div className="authoring-assist-body">{children}</div>
    </section>
  );
}
