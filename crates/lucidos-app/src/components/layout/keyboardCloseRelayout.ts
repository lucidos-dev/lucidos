/** Relayout the shell when the software keyboard closes.
 *
 *  A FIX, not a diagnostic. On an iOS PWA the page sometimes stops receiving
 *  touches entirely once the keyboard dismisses. Layout and `visualViewport`
 *  both report the restored viewport, so every reading the page can take says
 *  healthy. Forcing a relayout frees it, and that is all this module does.
 *
 *  The eighteenth report is where the keyboard close was first read as the
 *  trigger. The page took no touch for 36 seconds, a relayout ran, and the next
 *  tap was served. The reconstruction is
 *  `docs/plans/2026-09-20-the-composer-recovers-when-the-keyboard-closes.md`.
 *
 *  It lives here rather than in `deadPressProbe.ts` because that module is a
 *  temporary measure, and deleting it must not delete this. A leaf: it imports
 *  nothing, so the probe can read its stamp without taking anything else. */

/** How close to the layout viewport counts as fully restored.
 *
 *  A visual viewport is quoted in fractional pixels, so a restored one lands a
 *  hair off the layout viewport rather than exactly on it. */
const RESTORED_SLACK_PX = 4;

/** How far below the layout viewport counts as covered by a keyboard.
 *
 *  Well above iOS's form accessory bar and its toolbars, and well below the
 *  keyboard itself, which takes about 376 px of an 852 px iPhone. A smaller
 *  drop is some other chrome, and its going is not the transition this
 *  answers. */
const KEYBOARD_COVER_MIN_PX = 100;

/** One reading of the viewport, as the transition decision compares them. */
export interface ViewportSample {
  /** `visualViewport.height`, which the keyboard shrinks. */
  height: number;
  /** `window.innerHeight`, which it does not. */
  layoutViewport: number;
}

/** What a run of readings has established, or null before a covered one.
 *
 *  The layout viewport the cover was seen in, once one has been. A rotation
 *  retires the watch, because readings taken in two geometries cannot be
 *  compared. */
export type CoverWatch = number | null;

/** Fold one reading into the watch, and say whether it completed a close.
 *
 *  The EDGE, never the state. A reading at an unchanged restored height
 *  answers false, so the caller spends one relayout per close.
 *
 *  The watch is what carries the cover across the animation. iOS reports an
 *  interactive dismissal as a run of intermediate heights, and a keyboard
 *  half-way out is neither covered nor restored. Comparing only with the
 *  PREVIOUS reading would see 820 before 852 and call that no keyboard at all.
 *  The close this exists for would then be lost.
 *
 *  Pure, so the whole sequence tests without a DOM. */
export function foldViewportReading(
  watch: CoverWatch,
  sample: ViewportSample,
): { watch: CoverWatch; closed: boolean } {
  const full = sample.layoutViewport;
  if (!Number.isFinite(full) || full <= 0 || !Number.isFinite(sample.height)) {
    return { watch, closed: false };
  }
  const live = watch === full ? watch : null;
  if (sample.height <= full - KEYBOARD_COVER_MIN_PX) return { watch: full, closed: false };
  if (sample.height < full - RESTORED_SLACK_PX) return { watch: live, closed: false };
  return { watch: null, closed: live !== null };
}

/** The height to relayout the shell at before putting it back.
 *
 *  The AMPLITUDE of the user's own recovery: the keyboard's own span, which is
 *  the layout viewport less the shell's current height. That span is one pixel
 *  once the keyboard is already down, and the ledger shows one pixel clearing
 *  the wedge.
 *
 *  DOWNWARD, though the keyboard bounce goes up, and that is deliberate.
 *  Growing the shell shrinks every scroller in it. The browser clamps their
 *  scroll offsets at that layout, and putting the height back does NOT put the
 *  offsets back. A transcript at the live edge would jump most of a screen.
 *  Shrinking cannot clamp anything: it only makes room, and the restore returns
 *  to geometry those offsets were already valid in.
 *
 *  Floored at 1px, since a shell of zero height is not a layout. */
export function bounceHeight(current: number, layoutViewport: number): number {
  const keyboard = Math.round(layoutViewport - current);
  const span = keyboard > 0 ? keyboard : 1;
  return Math.max(1, Math.round(current - span));
}

/** Force the shell to relayout, which is what the user's own recovery does.
 *
 *  Closing and reopening the keyboard rewrites the visual viewport, and the app
 *  answers by rewriting `--app-height`. So nudging that property and putting it
 *  straight back reproduces the effect, without touching focus, the caret or
 *  the keyboard. Blurring the textarea would dismiss the keyboard, and iOS
 *  refuses to reopen it outside a user gesture.
 *
 *  Both writes happen in one task, so nothing is painted in between and the
 *  nudge is invisible. Reading `offsetHeight` between them is what makes each
 *  write a real layout rather than a coalesced no-op.
 *
 *  False when the property is not set, which is a shell this does not own and
 *  must not start writing. */
export function relayoutShell(): boolean {
  const root = document.documentElement;
  const prior = root.style.getPropertyValue('--app-height');
  // A px length, not merely something starting with a number. Writing `99px`
  // over a `100%` would change the unit for the instant before the restore.
  if (!/^-?[\d.]+px$/.test(prior.trim())) return false;
  const px = Number.parseFloat(prior);
  if (!Number.isFinite(px)) return false;
  const inner = window.innerHeight;
  const away = Number.isFinite(inner) && inner > 0 ? bounceHeight(px, inner) : Math.max(1, px - 1);
  root.style.setProperty('--app-height', `${away}px`);
  void root.offsetHeight;
  root.style.setProperty('--app-height', prior);
  void root.offsetHeight;
  // Recompute the layout viewport too. A no-op scroll, since it asks for the
  // offset the page already holds.
  if (typeof window.scrollTo === 'function') window.scrollTo(0, window.scrollY);
  return true;
}

/** What the readings so far have established. See `foldViewportReading`. */
let watch: CoverWatch = null;

/** When the keyboard last closed, and whether that close relaid the shell out.
 *
 *  Read by the composer's press ledger, which cannot see the resize itself. Its
 *  silence verdict describes the stretch after a close, so it needs the stamp
 *  and nothing else from here. */
let closedAt: number | null = null;
let closeRelaidOut = false;
let closes = 0;

export interface KeyboardCloseState {
  /** When the last close was seen, or null if none has been. */
  at: number | null;
  /** Whether that close's own relayout ran. False says `--app-height` was not
   *  ours to write at that moment, which rules the recovery out rather than
   *  scoring it. */
  relaidOut: boolean;
  /** Closes seen since the page loaded. A running total, so a reader can count
   *  the ones inside any stretch by subtracting the two ends. */
  closes: number;
}

export function keyboardCloseState(): KeyboardCloseState {
  return { at: closedAt, relaidOut: closeRelaidOut, closes };
}

/** Take a viewport reading, and relayout when it restored a covered viewport.
 *
 *  Called from the app's own `visualViewport` resize handler, AFTER that
 *  handler has written the restored `--app-height`. The ordering is why this is
 *  a call rather than a second listener. The property keeps one owner, and the
 *  bounce starts from the height the owner just settled on. */
export function noteViewportResize(sample: ViewportSample, now = Date.now()): void {
  const folded = foldViewportReading(watch, sample);
  watch = folded.watch;
  if (!folded.closed) return;
  closedAt = now;
  closes += 1;
  closeRelaidOut = relayoutShell();
}

/** Forget every reading, so one test's transition cannot reach the next. */
export function resetKeyboardCloseState(): void {
  watch = null;
  closedAt = null;
  closeRelaidOut = false;
  closes = 0;
}
