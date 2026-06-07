import { useCallback, useEffect, useRef } from "react";
import type { FormEvent, KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";

import { IconButton } from "./IconButton";
import { XIcon } from "./icons";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Shared modal-dialog primitive. Owns the backdrop + card, Escape-to-close,
 *  click-outside-to-close, a focus trap with initial focus (the first
 *  [data-autofocus] element, else the first focusable) and focus restore on
 *  unmount. Consumers supply the title, body (children), and footer actions.
 *
 *  Deliberately NO global Enter-to-confirm: a destructive confirm must be an
 *  explicit click (the old ConfirmDialogHost bound Enter→confirm, so a stray
 *  Enter could fire a delete). Confirmation affordance is the focused button. */
interface ModalProps {
  title: ReactNode;
  onClose: () => void;
  children: ReactNode;
  /** Right-aligned action buttons (in .modal-actions). */
  footer?: ReactNode;
  /** alertdialog for destructive confirmations; dialog otherwise. */
  role?: "dialog" | "alertdialog";
  /** Accessible name for the dialog. */
  ariaLabel: string;
  /** Extra class on the .modal card (per-dialog sizing). */
  className?: string;
  /** Extra class on the .modal-body. */
  bodyClassName?: string;
  /** Show a header close (×) button. */
  closeButton?: boolean;
  /** Render body+footer inside a <form> with this submit handler. */
  onSubmit?: (e: FormEvent) => void;
}

export function Modal({
  title,
  onClose,
  children,
  footer,
  role = "dialog",
  ariaLabel,
  className,
  bodyClassName,
  closeButton = false,
  onSubmit,
}: ModalProps) {
  const cardRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const prev = document.activeElement as HTMLElement | null;
    const card = cardRef.current;
    const target =
      card?.querySelector<HTMLElement>("[data-autofocus]") ??
      card?.querySelector<HTMLElement>(FOCUSABLE) ??
      card;
    target?.focus();
    return () => prev?.focus?.();
  }, []);

  const onKeyDown = useCallback(
    (e: ReactKeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
        return;
      }
      if (e.key !== "Tab") return;
      const card = cardRef.current;
      if (!card) return;
      const items = Array.from(card.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
        (el) => el.offsetParent !== null,
      );
      if (items.length === 0) return;
      const first = items[0];
      const last = items[items.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    },
    [onClose],
  );

  const body = (
    <>
      <div className={["modal-body", bodyClassName].filter(Boolean).join(" ")}>{children}</div>
      {footer !== undefined ? <div className="modal-actions">{footer}</div> : null}
    </>
  );

  return (
    <div className="modal-backdrop" onMouseDown={onClose}>
      <div
        ref={cardRef}
        className={["modal", className].filter(Boolean).join(" ")}
        role={role}
        aria-modal="true"
        aria-label={ariaLabel}
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={onKeyDown}
      >
        <header className="modal-header">
          <h2>{title}</h2>
          {closeButton ? (
            <IconButton label="Close" icon={<XIcon />} className="modal-close" onClick={onClose} />
          ) : null}
        </header>
        {onSubmit ? <form onSubmit={onSubmit}>{body}</form> : body}
      </div>
    </div>
  );
}
