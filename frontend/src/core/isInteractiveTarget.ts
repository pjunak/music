/** Returns true if the event originated in a place where shortcuts should
 *  not preempt typing OR steal arrow-key seeks from a focused custom
 *  control. Inputs, textareas, contenteditable, native form controls, and
 *  ARIA sliders (the seek bar is a tabindex'd <div role="slider"> with its
 *  own onKeyDown handler — we don't want the global ←/→ to also fire
 *  prev/next while the operator is scrubbing). */
export function isInteractiveTarget(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const tag = target.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  if (target.isContentEditable) return true;
  if (target.getAttribute("role") === "slider") return true;
  return false;
}
