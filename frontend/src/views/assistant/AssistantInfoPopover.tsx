import type { ReactNode } from "react";

interface AssistantInfoPopoverProps {
  label: string;
  title: string;
  children: ReactNode;
  className?: string;
}

export function AssistantInfoPopover({
  label,
  title,
  children,
  className = "",
}: AssistantInfoPopoverProps) {
  return (
    <details className={`assistant-info-popover ${className}`.trim()}>
      <summary>
        <span aria-hidden="true">i</span>
        <span>{label}</span>
      </summary>
      <div className="assistant-info-popover-panel">
        <strong className="assistant-info-popover-title">{title}</strong>
        {children}
      </div>
    </details>
  );
}

interface ProviderBoundaryPopoverProps {
  shared: string[];
  neverShared: string[];
  footer?: ReactNode;
  children?: ReactNode;
  sharedLabel?: string;
  label?: string;
}

export function ProviderBoundaryPopover({
  shared,
  neverShared,
  footer,
  children,
  sharedLabel = "Shared after confirmation",
  label = "Provider boundary",
}: ProviderBoundaryPopoverProps) {
  return (
    <AssistantInfoPopover
      label={label}
      title="What leaves the server"
      className="assistant-provider-boundary-popover"
    >
      <div className="assistant-info-boundary-grid">
        <div>
          <strong>{sharedLabel}</strong>
          <ul>
            {shared.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
        <div>
          <strong>Stays here</strong>
          <ul>
            {neverShared.map((item) => (
              <li key={item}>{item}</li>
            ))}
          </ul>
        </div>
      </div>
      {children}
      {footer ? <p className="assistant-info-popover-footer">{footer}</p> : null}
    </AssistantInfoPopover>
  );
}
