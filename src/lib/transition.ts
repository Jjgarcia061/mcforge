import { flushSync } from "react-dom";

/**
 * Applies a state update inside a View Transition so elements with a
 * `view-transition-name` animate in, out and to their new position.
 * Falls back to an instant update where unsupported or when the user prefers less motion.
 */
export function withViewTransition(update: () => void) {
  if (!document.startViewTransition || matchMedia("(prefers-reduced-motion: reduce)").matches) {
    update();
    return;
  }
  document.startViewTransition(() => flushSync(update));
}
