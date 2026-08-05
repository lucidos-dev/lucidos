/** Tab focus trap for the two centered dialogs (`ConfirmDialog`,
 *  `PromptDialog`). Both render a small `<Overlay>` panel whose whole point is
 *  that the keyboard cannot leave it until the user answers, and both hand-rolled
 *  the identical wrap logic before this was extracted.
 *
 *  The boundary decision itself is NOT re-derived here: it is
 *  `trapTargetIndex` from `layout/paneFocus.ts`, the same pure kernel the pane
 *  trap uses, already covered by that module's unit tests. What differs between
 *  the two traps is everything AROUND that kernel, and only that:
 *
 *  - the pane trap is anchored on the `focusedPane` signal and pulls focus IN
 *    when it is outside the pane; a dialog has no signal to anchor on and only
 *    wraps at its two boundaries.
 *  - the pane trap's `FOCUSABLE` excludes every explicitly `tabindex="-1"`
 *    control, for the drawer's mouse-only row buttons. A dialog contains only
 *    its own controls and has no such rows, so its set stays looser.
 */
import { trapTargetIndex } from '../layout/paneFocus';

/** Tabbable elements inside a dialog panel, in DOM order. Deliberately looser
 *  than the pane trap's set: a dialog contains only its own controls, so the
 *  native-focusable terms need no per-term `[tabindex="-1"]` exclusion. */
const DIALOG_FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

/** Wrap Tab / Shift+Tab at the dialog's boundaries so focus stays inside.
 *  Call from the dialog's `keydown` handler; it ignores every key but Tab, and
 *  leaves the in-between steps to the browser's own tab order. */
export function trapDialogTab(e: KeyboardEvent, root: HTMLElement | null): void {
  if (e.key !== 'Tab' || !root) return;
  const focusables = Array.from(root.querySelectorAll<HTMLElement>(DIALOG_FOCUSABLE));
  const active = document.activeElement as HTMLElement | null;
  // `indexOf` of a non-member is -1, which `trapTargetIndex` already reads as
  // "never wrap", so an active element outside the dialog needs no guard here.
  const target = trapTargetIndex(focusables.length, focusables.indexOf(active as HTMLElement), e.shiftKey);
  if (target === null) return;
  e.preventDefault();
  focusables[target].focus();
}
