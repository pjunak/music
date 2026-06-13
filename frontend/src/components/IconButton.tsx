import type { ButtonHTMLAttributes, ReactNode } from "react";

/** Icon-with-label button.
 *
 *  Use this for any button whose visible content is an icon (or icon +
 *  short text). The single `label` prop is required and powers BOTH the
 *  hover tooltip (`title`) and the accessible name (`aria-label`), so
 *  there's no way to ship an unlabeled icon button by accident.
 *
 *  Variants forward to the existing global classes (`btn-primary` etc.)
 *  so this slots into the design system without adding new colour rules.
 *
 *  Padding/sizing comes from context (`.col-actions button`, etc.) when
 *  the button sits inside a styled container — this component only adds
 *  layout (flex-centred icon + optional text) and the label glue. */

interface Props extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label" | "title"> {
  /** Required: used as both `title` and `aria-label`. */
  label: string;
  /** Icon node — typically a component from `./icons`. */
  icon: ReactNode;
  /** Optional visible text after the icon. */
  children?: ReactNode;
  variant?: "default" | "primary" | "secondary" | "danger" | "ghost";
}

export function IconButton({
  label,
  icon,
  children,
  variant = "default",
  className,
  type = "button",
  ...rest
}: Props) {
  const variantClass =
    variant === "primary"
      ? "btn-primary"
      : variant === "secondary"
        ? "btn-secondary"
        : variant === "danger"
          ? "btn-danger"
          : variant === "ghost"
            ? "btn-ghost"
            : "";
  const composed = ["icon-button", variantClass, className]
    .filter(Boolean)
    .join(" ");
  return (
    <button type={type} className={composed} title={label} aria-label={label} {...rest}>
      <span className="icon-button-glyph" aria-hidden="true">
        {icon}
      </span>
      {children !== undefined ? <span>{children}</span> : null}
    </button>
  );
}
