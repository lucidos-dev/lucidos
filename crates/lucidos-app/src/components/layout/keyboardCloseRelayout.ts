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
 *  `covered` is the opposite edge, and a stamp is retired on it. A stale close
 *  described a keyboard that had come back up, and the ledger line naming it
 *  read the opposite of the viewport beside it.
 *
 *  Pure, so the whole sequence tests without a DOM. */
export function foldViewportReading(
  watch: CoverWatch,
  sample: ViewportSample,
): { watch: CoverWatch; closed: boolean; covered: boolean } {
  const full = sample.layoutViewport;
  if (!Number.isFinite(full) || full <= 0 || !Number.isFinite(sample.height)) {
    return { watch, closed: false, covered: false };
  }
  const live = watch === full ? watch : null;
  if (sample.height <= full - KEYBOARD_COVER_MIN_PX) {
    return { watch: full, closed: false, covered: true };
  }
  if (sample.height < full - RESTORED_SLACK_PX) {
    return { watch: live, closed: false, covered: false };
  }
  return { watch: null, closed: live !== null, covered: false };
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
let closePath: ClosePath | null = null;

/** How long a covered reading after a wake is taken for a stale echo.
 *
 *  iOS leaves `visualViewport.height` pinned at the shrunk value for a moment
 *  after a resume, and corrects it on a delayed resize. Folding that echo would
 *  arm a second close on the correction.
 *
 *  BOUNDED, and the bound is the whole point. An unbounded suppression clears
 *  only on a reading it accepts. A keyboard the user genuinely reopens is then
 *  swallowed, and its real close is lost.
 *
 *  SHORT, because the interval it has to cover is short. It is the resume to
 *  the correcting resize, which lands in well under a second. It is NOT the
 *  resume to the next dismissal, which can be minutes. A tap that reopens the
 *  keys inside the window still loses its close, so every millisecond here is
 *  hole. Erring long loses a close, and erring short over-counts one at the
 *  cost of a spare invisible relayout. */
const WAKE_ECHO_MS = 600;
let wakeEchoUntil = 0;

/** Which path saw a close. Defined in `docs/glossary.md` § Close path. */
export type ClosePath = 'resize' | 'wake' | 'poll';

export interface KeyboardCloseState {
  /** When the last close was seen, or null if none has been. Null again once
   *  the keyboard comes back up, so a silence verdict cannot cite a close the
   *  viewport has already contradicted. */
  at: number | null;
  /** Whether that close's own relayout ran. False says `--app-height` was not
   *  ours to write at that moment, which rules the recovery out rather than
   *  scoring it. */
  relaidOut: boolean;
  /** Closes seen since the page loaded. A running total, so a reader can count
   *  the ones inside any stretch by subtracting the two ends. */
  closes: number;
  /** Which path saw the last close. See `ClosePath`. */
  path: ClosePath | null;
}

export function keyboardCloseState(): KeyboardCloseState {
  return { at: closedAt, relaidOut: closeRelaidOut, closes, path: closePath };
}

/** Stamp a close and spend its relayout. The one place any path lands. */
function recordClose(path: ClosePath, now: number): void {
  closedAt = now;
  closePath = path;
  closes += 1;
  closeRelaidOut = relayoutShell();
}

/** Drop a stamp the keyboard has outlived. `closes` is a total and stays. */
function retireClose(): void {
  closedAt = null;
  closeRelaidOut = false;
  closePath = null;
}

/** Fold one reading from whichever path took it. */
function takeReading(sample: ViewportSample, path: ClosePath, now: number): void {
  const folded = foldViewportReading(watch, sample);
  // See `WAKE_ECHO_MS`. Dropped rather than folded, so the echo cannot arm a
  // watch and turn its own correction into a second close.
  if (folded.covered && now < wakeEchoUntil) return;
  watch = folded.watch;
  if (folded.covered) retireClose();
  if (folded.closed) recordClose(path, now);
}

/** Take a viewport reading, and relayout when it restored a covered viewport.
 *
 *  Called from the app's own `visualViewport` resize handler, AFTER that
 *  handler has written the restored `--app-height`. The ordering is why this is
 *  a call rather than a second listener. The property keeps one owner, and the
 *  bounce starts from the height the owner just settled on. */
export function noteViewportResize(sample: ViewportSample, now = Date.now()): void {
  takeReading(sample, 'resize', now);
}

/** A reading nobody asked for, taken on a timer.
 *
 *  The backstop for a close no event announced. A wedged page keeps its timers
 *  while it takes no touch, which is what makes a reading possible where an
 *  event is not. Same contract as `noteViewportResize`: the caller settles
 *  `--app-height` first, so the bounce starts from the height it just wrote. */
export function notePolledViewport(sample: ViewportSample, now = Date.now()): void {
  takeReading(sample, 'poll', now);
}

/** Open a resume's echo window, and say whether this call is a twin to skip.
 *
 *  A twin needs BOTH an open window and a close already standing. The corrected
 *  half reports only an edge it observed, so it finds nothing when no cover was
 *  armed. That is the wedge case: no resize ever fired, so no cover exists. The
 *  pinned half must still land its close there, and the stamp test is what lets
 *  it.
 *
 *  BOTH halves of the wake go through here, and the ordering is why. One resume
 *  fires `visibilitychange` AND `pageshow`, so the caller runs twice, and iOS
 *  can hand the two events different viewports. A window only the pinned half
 *  opened leaves corrected-then-pinned unguarded, which is two closes for one
 *  resume.
 *
 *  Scoped to THAT PAIR, and no wider. The two land milliseconds apart, so the
 *  window bounds them. A guard on the outstanding stamp alone swallows every
 *  later resume too. The wedge this answers is the state where no reading
 *  arrives to retire a stamp, so one can stand indefinitely. */
function openWakeWindow(now: number): boolean {
  const paired = now < wakeEchoUntil;
  wakeEchoUntil = now + WAKE_ECHO_MS;
  return paired && closedAt !== null;
}

/** A resume whose viewport came back already corrected.
 *
 *  The other half of the wake, and it needs no verdict: the restored reading IS
 *  the edge the fold has waited for since the keys went up. Only the pinned
 *  half below has none to offer.
 *
 *  Its own path, so the ledger says a wake caught it either way. */
export function noteWakeViewport(sample: ViewportSample, now = Date.now()): void {
  if (openWakeWindow(now)) return;
  takeReading(sample, 'wake', now);
}

/** The keys went and no resize said so.
 *
 *  For the wake path, which restores the shell itself. iOS dismisses the
 *  keyboard across a suspend and fires no resize, so the fold never sees the
 *  edge and the recovery never runs. The caller has already ruled the keys
 *  gone, so this takes its word rather than a viewport reading. */
export function noteKeyboardClosed(now = Date.now()): void {
  if (openWakeWindow(now)) return;
  watch = null;
  recordClose('wake', now);
}

/** Forget every reading, so one test's transition cannot reach the next. */
export function resetKeyboardCloseState(): void {
  watch = null;
  closedAt = null;
  closeRelaidOut = false;
  closes = 0;
  closePath = null;
  wakeEchoUntil = 0;
}
