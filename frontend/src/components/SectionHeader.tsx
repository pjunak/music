import type { ReactNode } from "react";

/** A panel/card/group heading: small uppercase-muted title on the left,
 *  optional trailing action(s) on the right. Replaces the ad-hoc
 *  space-between header divs and the bare <h3>s that fell through to UA
 *  styling. */
export function SectionHeader({
  title,
  actions,
  className,
}: {
  title: ReactNode;
  actions?: ReactNode;
  className?: string;
}) {
  return (
    <div className={["section-header", className].filter(Boolean).join(" ")}>
      <h3 className="section-label">{title}</h3>
      {actions !== undefined ? <div className="header-actions">{actions}</div> : null}
    </div>
  );
}
