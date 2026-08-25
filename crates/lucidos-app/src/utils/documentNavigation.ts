/**
 * Navigation from one Lucidos document to another: workspace to workspace,
 * picker to workspace, workspace to picker.
 *
 * Every such navigation REPLACES the current history entry, and this module owns
 * that rule for the ones a user triggers. The boot path already keeps the back
 * stack flat on its own: `navigateToPane` never pushes, and the cold-start
 * redirect in `index.html`, `recoverFromBrokenContext` (`main.tsx`) and
 * `bounceToPickerIfStranded` (`store/actions/connection.ts`) all replace.
 *
 * A flat stack is what makes a stray back gesture harmless. On iOS, a swipe
 * right from the left screen edge IS the back gesture. A standalone PWA can only
 * suppress it by cancelling the touchstart, and that guard
 * (`shouldSuppressEdgeNavigation`) cannot cover every touch. Leave a workspace
 * on the back stack and each hole in it becomes a silent teleport into that
 * workspace. The plan has the full reasoning:
 * `docs/plans/2026-08-21-workspace-navigation-never-pushes-history.md`.
 */

// Leaving the app entirely is a different job, and `utils/openExternalUrl.ts`
// owns it. That file holds the one sanctioned `location.href` assignment in the
// frontend, pinned by the source scan in `documentNavigation.test.ts`.

/** Navigate this document to `href`, overwriting the current history entry. */
export function replaceDocument(href: string): void {
  window.location.replace(href);
}

/** The parts of a click that decide whether a handler may take it over. Named
 *  structurally rather than as a `MouseEvent` so the decision is testable
 *  without a DOM, matching `shouldSuppressEdgeNavigation`. */
export interface ClickModifiers {
  button?: number;
  metaKey?: boolean;
  ctrlKey?: boolean;
  shiftKey?: boolean;
  altKey?: boolean;
  defaultPrevented?: boolean;
}

/** Pure decision: is this the plain left click a link handler may replace?
 *
 *  Everything else belongs to the browser. Cmd, Ctrl, Shift and middle clicks
 *  open a new tab or window. Taking those over would break the one thing an
 *  `<a href>` gives a user over a button. An already-cancelled click is somebody
 *  else's, so leave it alone. */
export function isPlainClick(e: ClickModifiers): boolean {
  if (e.defaultPrevented) return false;
  if (e.button !== undefined && e.button !== 0) return false;
  return !e.metaKey && !e.ctrlKey && !e.shiftKey && !e.altKey;
}

/** Click handler for an `<a href>` pointing at another Lucidos document: keep
 *  the anchor's semantics, but make a plain click replace instead of push.
 *
 *  `before` runs on every click, modifier clicks included. It is the caller's
 *  own bookkeeping, shutting the menu the link sits in, and a click that opens a
 *  new tab dismissed that menu too. */
export function replaceOnPlainClick(href: string, before?: () => void) {
  return (e: MouseEvent): void => {
    before?.();
    if (!isPlainClick(e)) return;
    e.preventDefault();
    replaceDocument(href);
  };
}
