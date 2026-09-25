import { showToast } from '../../store/store';
import { isMobile, isTouchDevice } from '../../utils/viewport';
import { TAP_MOVE_THRESHOLD_PX, takePressOutcome, type PressOutcome } from '../../utils/tapGesture';
import { postClientLog } from '../../utils/clientLog';
import { relayoutShell, keyboardCloseState, type ClosePath } from '../layout/keyboardCloseRelayout';
import { readViewport, type ProbeViewport } from './probeViewport';

// Re-exported so this module stays the one import for a press report's types.
// The reading itself is shared with the keystroke probe, which is why it moved.
export type { ProbeViewport };

/** Reports a tap on a composer action button that produced nothing.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1 and removed once
 *  a report names the cause. The bug behind it has been reported twenty times
 *  and nobody can reproduce it: it strikes now and then on an iOS PWA and kills
 *  the composer's buttons wherever the finger presses. No emulator reproduces
 *  it, so the app has to be the one that says what happened.
 *
 *  The eighth episode was SILENT, and that is what this module is shaped by. The
 *  probe used to arm only from a `touchstart` it could attribute to the row. A
 *  gesture the page never received therefore left no trace at all. It now
 *  partitions the ways a press can go missing, and the plan behind that
 *  partition is
 *  `docs/plans/2026-08-29-the-composer-says-when-send-is-unreachable.md`.
 *
 *  EVIDENCE, plus one recovery. The relayout that answers a keyboard close is a
 *  fix and lives outside this module, in `layout/keyboardCloseRelayout.ts`, so
 *  deleting the probe cannot delete it. What stays here is the ledger and the
 *  typing-driven relayout the ledger scores.
 *
 *  Two apparatuses were cut with the eighteenth report. The machinery that
 *  decided whether to press Send went with the commit ADR 0225 retired. The
 *  reachability repair went because it answered healthy through every episode
 *  it was built for. Both removals are named in
 *  `docs/plans/2026-09-20-the-composer-recovers-when-the-keyboard-closes.md`.
 *
 *  The decisions are pure functions of what was measured, so they test without a
 *  DOM. `installDeadPressProbe` is the shell that measures.
 *
 *  TWO output channels, and the log is the one to trust. `recordPress` writes
 *  every watched press to `engine.log`, whatever it ended as, so an episode can
 *  be read back from the workspace afterwards. The toast is the other half: a
 *  `warning`, which `showToast` holds until the user dismisses it, for the
 *  presses that died with nothing to explain them.
 *
 *  The log exists because the toast alone kept failing. Five episodes produced
 *  five reports reading "nothing happened", because a toast reports only to
 *  whoever is looking at the screen and keeps it. */

/** A box in the same client space `elementFromPoint` and a touch's `clientX`
 *  are quoted in. */
export interface ProbeRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface LandingFacts {
  /** The pressed face's name, so the report says which button died. */
  face: string;
  /** Where the finger touched down. */
  point: { x: number; y: number };
  /** The face's box, or null when it is not on screen. */
  faceRect: ProbeRect | null;
  /** Whether the `touchstart` was dispatched to the face, or inside it. */
  targetIsFace: boolean;
  /** What the browser reports at `point`, for the disagreement case. */
  elementAtPoint: string | null;
  /** The computed `pointer-events` of whatever answered at `point`. `none` there
   *  names an inert overlay outright, which no coordinate can. */
  pointerEventsAtPoint: string | null;
  viewport: ProbeViewport;
}

/** How many contacts the glass held across one press.
 *
 *  `fingers` is the most `TouchEvent.touches` ever reported while the press was
 *  armed, so a finger joining halfway through still counts. `fingersAtLift` is
 *  what remained down when this press ended, which is zero for a press nobody
 *  shared. */
export interface PressFingers {
  fingers: number;
  fingersAtLift: number;
}

/** Was this press the only contact on the glass, start to finish?
 *
 *  WebKit synthesises no click for a multi-finger gesture, so a press sharing
 *  the glass produces exactly what a dead one does: a `touchstart`, a `touchend`
 *  and nothing else. The seventeenth report is where that cost a false alarm.
 *  An Archive press was called dead with `quiet.ms` of 13, which places a second
 *  contact 13ms in front of it. Every report below is therefore silent unless
 *  the press was alone. */
export function pressWasAlone(f: PressFingers): boolean {
  return f.fingers <= 1 && f.fingersAtLift === 0;
}

export interface DeadPressFacts {
  face: string;
  /** How far the finger travelled, in screen px. */
  movedPx: number;
  /** `Node.isConnected` on the pressed element, read at `touchend`. */
  connectedAtLift: boolean;
  /** DOM mutations seen inside the prompt actions row across the press. */
  rowMutations: number;
  /** Who claimed the press, from `takePressOutcome`. Null when nobody did,
   *  which is the dead press this module exists for. */
  outcome: PressOutcome | null;
  /** See `pressWasAlone`. */
  alone: boolean;
  viewport: ProbeViewport;
}

function inside(rect: ProbeRect, x: number, y: number): boolean {
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

function viewportSuffix(v: ProbeViewport): string {
  return `vv ${Math.round(v.vvHeight)} +${Math.round(v.vvOffsetTop)}, `
    + `inner ${Math.round(v.innerHeight)}, app ${v.appHeight || 'unset'}, `
    + `kbd ${v.keyboardActive ? 'on' : 'off'}, `
    + `scroll ${Math.round(v.pageScrollY)}`;
}

/** Did the press land on the pixels the button is painted on, yet go somewhere
 *  else? That is the browser hit-testing against a layout it is no longer
 *  painting, and the offset is the evidence. Null when nothing is wrong. */
export function landingReport(f: LandingFacts): string | null {
  if (!f.faceRect || f.targetIsFace) return null;
  if (!inside(f.faceRect, f.point.x, f.point.y)) return null;
  const centreY = Math.round((f.faceRect.top + f.faceRect.bottom) / 2);
  return `${f.face} did not register: the tap was on the button but the browser `
    + `sent it to ${f.elementAtPoint ?? 'nothing'} `
    + `(pointer-events ${f.pointerEventsAtPoint ?? 'unknown'}). `
    + `Button centre y ${centreY}, touch y ${Math.round(f.point.y)}. `
    + viewportSuffix(f.viewport);
}

/** The press reached the button and no path took it. Three shapes, ordered by
 *  how much they settle.
 *
 *  A button that left the document under the finger is the whole answer: the
 *  `touchend` went to a detached node and the cascade was stopped. A button that
 *  survived while the row churned is the same mechanism one step weaker. Neither
 *  means both activation paths declined on a stable node.
 *
 *  Null when the finger MOVED past the tap threshold, which is a press the
 *  platform is entitled to drop. A click-only face produces no click when the
 *  press slides off it, by design, so reporting that would toast through
 *  ordinary use. Stop, Cancel and every banner action are click-only. The fault
 *  being chased is a stationary tap going dead.
 *
 *  Null too for a press somebody CLAIMED, and both claims are legitimate. A
 *  served press ran its button's action. A swallowed one is the overlay
 *  contract: a tap that dismisses a popover must not also press what was under
 *  it, and Send is a thing that can be under it. So neither toasts. Both still
 *  reach the log, which is where a run of swallowed Sends with no popover open
 *  would name its own cause.
 *
 *  Null too for a press that shared the glass. See `pressWasAlone`. */
export function deadPressReport(f: DeadPressFacts): string | null {
  if (f.outcome !== null) return null;
  if (!f.alone) return null;
  if (f.movedPx > TAP_MOVE_THRESHOLD_PX) return null;
  const tail = ` ${viewportSuffix(f.viewport)}`;
  if (!f.connectedAtLift) {
    return `${f.face} did not register: the button was replaced in the page `
      + `while your finger was on it (${f.rowMutations} row changes).` + tail;
  }
  if (f.rowMutations > 0) {
    return `${f.face} did not register: the button survived, but the row `
      + `changed ${f.rowMutations} times during the press.` + tail;
  }
  return `${f.face} did not register: the press reached the button and nothing `
    + `ran.` + tail;
}

/** The system took the gesture, so no path ran and no click followed.
 *
 *  Silent when the finger moved, because a cancelled scroll is the platform
 *  working. Only a stationary press that the system still took is the fault
 *  being chased. Silent too when the press shared the glass, on the same
 *  grounds: a pinch is a gesture the system is entitled to take. */
export function canceledPressReport(f: {
  face: string;
  movedPx: number;
  alone: boolean;
  viewport: ProbeViewport;
}): string | null {
  if (!f.alone) return null;
  if (f.movedPx > TAP_MOVE_THRESHOLD_PX) return null;
  return `${f.face} did not register: the system cancelled the touch after `
    + `${Math.round(f.movedPx)}px. ${viewportSuffix(f.viewport)}`;
}

/** The press arrived and the lift never did. WebKit owes a `touchend` or a
 *  `touchcancel` for every `touchstart`, so neither arriving is the touch
 *  pipeline stopping mid-gesture.
 *
 *  That is the shape the eighth episode's silence points at. The old probe
 *  dropped such a press at the next `touchstart`, timer and all, so it produced
 *  no line at all.
 *
 *  Null for a finger that travelled, on the same threshold as every other
 *  report here: a gesture the page handed to a scroller is the platform
 *  working. Null for a press that shared the glass, where only the count at the
 *  press is known: the lift that would have given the other half never came. */
export function noLiftReport(f: {
  face: string;
  movedPx: number;
  alone: boolean;
  viewport: ProbeViewport;
}): string | null {
  if (!f.alone) return null;
  if (f.movedPx > TAP_MOVE_THRESHOLD_PX) return null;
  return `${f.face} did not register: the touch began on the button and the `
    + `lift never arrived. ${viewportSuffix(f.viewport)}`;
}

/** How long after `touchend` a `click` counts as belonging to that press.
 *  Longer than WebKit's synthetic-click delay, short enough that the toast still
 *  belongs to the tap the user just made. */
const CLICK_GRACE_MS = 600;

/** How long a `click` still counts as having a touch behind it.
 *
 *  Generous next to `CLICK_GRACE_MS`, because this window is not deciding a
 *  verdict for a press. It is deciding whether the page saw ANY touch recently,
 *  and a slow synthetic click must not be mistaken for a touchless one. */
const TOUCH_BEHIND_CLICK_MS = 1500;

/** What the press ended as. `served` and `swallowed` come from whoever took it;
 *  the rest are the probe's own readings.
 *
 *  `no-lift` and `click-no-touch` are the two halves of a touch pipeline that
 *  stopped: a gesture that began and never finished, and a click arriving with
 *  no gesture behind it at all.
 *
 *  `multi-touch` is a press that shared the glass, which is `dead` without the
 *  fault. */
type PressVerdict =
  | PressOutcome
  | 'dead'
  | 'multi-touch'
  | 'clicked'
  | 'canceled'
  | 'missed'
  | 'no-lift'
  | 'click-no-touch'
  | 'keyboard-touch'
  | 'covered'
  | 'stray-click'
  | 'untouched'
  | 'silent-since-keyboard';

/** How often a touch that reached nowhere near the composer may be logged.
 *
 *  Slow enough that a scroll or a flick writes one line, not a stream. */
const STRAY_TOUCH_THROTTLE_MS = 250;

/** How long a press may stay armed before the lift is called lost.
 *
 *  Far beyond any tap, and beyond the long press that reveals a tooltip, so an
 *  ordinary gesture always lifts first. Short enough that the report still
 *  reaches a user who is looking at the screen wondering why nothing happened. */
const LIFT_DEADLINE_MS = 4000;

/** A rect as the log carries it: whole pixels, and only the four edges.
 *
 *  Rounded because a sub-pixel layout value is noise against a finger, and
 *  because every line pays for its own width under the engine's 4KB cap. */
function roundRect(rect: ProbeRect | null): ProbeRect | null {
  if (!rect) return null;
  return {
    left: Math.round(rect.left),
    right: Math.round(rect.right),
    top: Math.round(rect.top),
    bottom: Math.round(rect.bottom),
  };
}

/** The quiet window that ended at the most recent input, and the counters
 *  filling the one now running.
 *
 *  The tenth episode wrote no line at all, which proves the page took no touch
 *  and no click while the composer sat dead. A probe that must be touched
 *  cannot report that, so the recovery input has to carry it instead.
 *
 *  Every line therefore says how long the page had been silent beforehand, and
 *  how many scheduled checks ran in that silence. The two read together
 *  bracket an episode. Riding on lines that are written anyway costs no new
 *  noise. */
interface QuietWindow {
  ms: number;
  checks: number;
  /** Checks that asked nothing, because the app's own cover was up.
   *
   *  The thirteenth episode is why. A cover makes every reading here decline,
   *  and declining used to be silent. So a stuck cover and a dead touch
   *  pipeline wrote the same nothing. This number tells them apart across a
   *  silence, which is the one stretch neither can be asked about. */
  covered: number;
  /** Relayouts the typing-driven recovery spent in this silence.
   *
   *  The only thing that can score that recovery. If it clears the wedge, the
   *  press ending the silence arrives with a nudge behind it and the composer
   *  still focused. If it does not, the press arrives after the user dismissed
   *  the keyboard by hand, exactly as it does today. See `shouldNudgeUntouched`. */
  nudges: number;
  /** Keyboard closes in this silence, each of which spent a relayout.
   *
   *  The transition-driven recovery's score, kept apart from `nudges` so that
   *  count keeps meaning exactly what every earlier ledger reads it as. A
   *  silence with a close in it had the fix run inside it. */
  closes: number;
}

let lastInputAt: number | null = null;
let checksSinceInput = 0;
let coveredSinceInput = 0;
let nudgesSinceInput = 0;
/** The running close total as the current silence opened. `quiet.closes` is the
 *  difference, so the probe needs no callback from the relayout. */
let closesAtInput = 0;
let quiet: QuietWindow | null = null;

/** When the composer's textarea last took a character.
 *
 *  Typing is not an input for the window above, deliberately: that reading is
 *  about touches and clicks, and a keystroke resetting it would erase the
 *  silence it brackets. This stamp is separate so the two never mix. */
let lastKeystrokeAt: number | null = null;

/** When a touch or a click last reached the COMPOSER, rather than the page.
 *
 *  What arms the typing-driven recovery, in place of `lastInputAt`. A touch
 *  landing elsewhere says nothing about whether the composer can be pressed,
 *  and round 15's ledger holds two of them inside one episode. See
 *  `COMPOSER_SELECTOR`. */
let lastComposerInputAt: number | null = null;

/** Relayouts the typing-driven recovery has spent since the last keystroke.
 *
 *  `quiet.nudges` cannot carry this, because it resets on every touch. The
 *  press that finally lands then reports zero whenever anything reached the
 *  page in between, which left round 15 unable to score the relayout. This
 *  count survives a touch, and rides every line beside the other. */
let nudgesSinceKeystroke = 0;

/** Close the running quiet window and open a fresh one. Called for every
 *  `touchstart` and every `click` the document sees, wherever they land. */
function noteInput(now: number): void {
  const { closes } = keyboardCloseState();
  if (lastInputAt !== null) {
    quiet = {
      ms: Math.round(now - lastInputAt),
      checks: checksSinceInput,
      covered: coveredSinceInput,
      nudges: nudgesSinceInput,
      closes: closes - closesAtInput,
    };
  }
  closesAtInput = closes;
  lastInputAt = now;
  checksSinceInput = 0;
  coveredSinceInput = 0;
  nudgesSinceInput = 0;
}

/** The state around a press, read at ONE instant.
 *
 *  The fourteenth episode is why this is a snapshot. A `missed` line takes its
 *  rects and its viewport inside the `touchstart` handler. A `served` line
 *  stored rects at touchdown and read the viewport 600ms after the lift. Each
 *  served line therefore held a row at one height and a viewport at another.
 *  The pair read as two rows, where it was two clocks.
 *
 *  `morph` is what made that legible, and only by accident: `absent` on the
 *  press and `disabled` on the same button's line two seconds later. It is a
 *  reading about the press, so it belongs to the press. */
interface PressContext {
  morph: MorphState;
  quiet: QuietWindow | null;
  /** See `nudgesSinceKeystroke`. Beside `quiet.nudges`, never replacing it:
   *  every past ledger is read against that field. */
  nudgesSinceKeystroke: number;
  viewport: ProbeViewport;
}

function pressContext(): PressContext {
  return {
    // The composer's own state. "The button is there, it just doesn't work" is
    // a claim about this, and no line has ever carried it.
    morph: readMorphState(),
    quiet,
    nudgesSinceKeystroke,
    viewport: readViewport(),
  };
}

/** The engine-log breadcrumb, written for EVERY press the probe watches.
 *
 *  A toast reports to whoever is looking at the screen and keeps it. That is
 *  how five episodes produced nothing to work from. This lands in `engine.log`
 *  instead, so an episode can be read back afterwards from the workspace.
 *
 *  It carries no draft text and no message content: what the user typed has no
 *  business in a log line (`.claude/rules/no-private-data.md`). */
function recordPress({ at, ...facts }: {
  face: string;
  verdict: PressVerdict;
  movedPx: number;
  connectedAtLift?: boolean;
  rowMutations?: number;
  elementAtPoint?: string | null;
  pointerEventsAtPoint?: string | null;
  toasted?: boolean;
  /** Where the row and the pressed face WERE, so a report measures a
   *  paint-versus-hit-test offset instead of implying one. */
  rowRect?: ProbeRect | null;
  faceRect?: ProbeRect | null;
  /** What sat under a finger no watchable face claimed, and the census behind
   *  that answer. Only the `missed` branch fills these. */
  under?: UnderFinger;
  underFace?: string | null;
  faceCount?: number;
  watchableCount?: number;
  /** Where the finger landed, and the nearest face it failed to reach. A near
   *  miss and a press nowhere near a target are one verdict without these. */
  point?: { x: number; y: number };
  /** The nearest face's BOX travels with the distance, and so does the SIGNED
   *  vector to it. A missed line's `faceRect` is null, which says only that no
   *  face held the point. `px` alone cannot tell a finger that landed off a
   *  face from a page hit-testing at an offset. `dx` and `dy` can, repeated
   *  across an episode. See `missVector`. */
  missedBy?: { face: string; px: number; dx: number; dy: number; rect: ProbeRect } | null;
  /** `screenOffset` for this press. The one reading not taken from the layout
   *  side, so a page hit-testing away from the glass says so here. */
  screenOff?: { x: number; y: number };
  /** Which cover the app had up when it declined to judge the press. Only the
   *  `covered` branch fills it. */
  cover?: string;
  /** Written with no GESTURE behind it. Two triggers qualify since round 20, so
   *  read `nudgeTrigger` for which one. */
  scheduled?: boolean;
  /** Which trigger spent the recovery: the 3s `tick`, or the `keystroke` timer
   *  armed when typing stops. Both write `untouched`, and attributing the
   *  relayout is the whole point of arming the second one. */
  nudgeTrigger?: NudgeTrigger;
  /** How long ago the keyboard closed. Only `silent-since-keyboard` fills it. */
  sinceKeyboardMs?: number;
  /** Which path saw that close. `poll` is the reading to look for: it says the
   *  page was never told, which is the mechanism this investigation chases. */
  closePath?: ClosePath | null;
  /** How long ago the document last took a touch or a click, and null when it
   *  has taken none at all. Read AGAINST `sinceKeyboardMs`: a smaller number
   *  means an input arrived after the close, and no other line named it. */
  sinceInputMs?: number | null;
  /** Whether the relayout actually ran. A line that never nudged rules the
   *  recovery out, rather than scoring it. */
  nudged?: boolean;
  /** How many contacts the glass held. See `PressFingers`. A line with no lift
   *  behind it carries the first alone. */
  fingers?: number;
  fingersAtLift?: number;
  /** The press's own context, for a line written after the press. Absent means
   *  NOW is the press, which is true of every line written inside a handler. */
  at?: PressContext;
}): void {
  postClientLog('composer-press', `${facts.face}: ${facts.verdict}`, {
    ...facts,
    movedPx: Math.round(facts.movedPx),
    ...(at ?? pressContext()),
  });
}

/** A touch that reached the page while the keyboard was up, and did NOT reach
 *  the composer's row.
 *
 *  The blind spot twelve episodes have died in. Every other line here needs the
 *  touch to be attributable to the composer, so "no line" has meant two
 *  opposite things at once: iOS delivered no touch at all, and iOS delivered it
 *  somewhere the composer is not.
 *
 *  Those two have different fixes and no shared one, which is why guessing
 *  between them has failed four times. A WKWebView that scrolled itself for the
 *  keyboard, and did not tell the page, displaces every touch by that amount.
 *  The page reads `scrollY` and `visualViewport.offsetTop` as zero throughout.
 *  This line says which: it carries where the finger was reported, what
 *  answered there, and the screen-to-client offset.
 *
 *  Throttled, and only while the keyboard is up, which is the only state any
 *  report describes. */
let lastStrayTouchAt = Number.NEGATIVE_INFINITY;

/** A touch as an UNRULED line carries it: where the finger was reported, what
 *  answered there, the offset between the two coordinate spaces, and the row it
 *  did not reach. Every verdict without a pressed face wants all four. */
type ProbeTouch = { clientX: number; clientY: number; screenX: number; screenY: number };

function touchLanding(t: ProbeTouch, rowRect: ProbeRect | null) {
  const point = { x: Math.round(t.clientX), y: Math.round(t.clientY) };
  return {
    point,
    screenOff: screenOffset(t),
    elementAtPoint: describe(document.elementFromPoint(point.x, point.y)),
    rowRect,
  };
}

/** Every watchable face with its box, the one read three questions share. */
function faceBoxes(): { name: string; rect: ProbeRect }[] {
  return watchableFaces().map((f) => ({
    name: nameOf(f),
    rect: roundRect(f.getBoundingClientRect()) as ProbeRect,
  }));
}

/** The wire shape for a near miss, so the two lines carrying it cannot drift. */
function missedByOf(nearest: ReturnType<typeof nearestFaceMiss<{ name: string; rect: ProbeRect }>>) {
  return nearest && {
    face: nearest.face.name,
    px: nearest.px,
    dx: nearest.dx,
    dy: nearest.dy,
    rect: nearest.face.rect,
  };
}

function noteStrayTouch(t: ProbeTouch, target: Element | null, rowRect: ProbeRect | null): void {
  if (!readViewport().keyboardActive) return;
  const now = Date.now();
  if (now - lastStrayTouchAt < STRAY_TOUCH_THROTTLE_MS) return;
  lastStrayTouchAt = now;
  const landing = touchLanding(t, rowRect);
  // How far this touch fell from the face it could have pressed.
  //
  // The reading round 20 found missing. This is the only line the wedge can
  // produce: a touch reaching the composer writes a different verdict, and one
  // reaching nothing writes none at all. So it is the one place a displacement
  // can show, and it carried the point without the distance.
  //
  // Read it as `missed` is read. A repeating dx and dy across an episode is
  // the page hit-testing away from the glass. Scatter is aim.
  //
  // Past the throttle and the keyboard test, so a healthy page pays no
  // geometry for it.
  //
  // Only against a row that HAS a box. This is also called where no row is laid
  // out. A zero-measuring face would then report a distance from the viewport
  // origin, which reads as the very displacement the vector exists to find.
  const nearest = rowRect ? nearestFaceMiss(faceBoxes(), landing.point) : null;
  recordPress({
    face: describe(target) ?? 'nothing',
    verdict: 'keyboard-touch',
    movedPx: 0,
    ...landing,
    missedBy: missedByOf(nearest),
  });
}

/** A touch that reached the composer's row while the app's own cover was up.
 *
 *  The third meaning a blank ledger has carried. `coveredOnPurpose` makes every
 *  reading below decline, and declining used to write nothing. So a press the
 *  probe REFUSED read exactly like a press that never arrived.
 *
 *  Only for a touch the probe would otherwise have ruled. A tap anywhere else
 *  under a cover is the user working the overlay. A line per menu item is the
 *  noise that made the eleventh round stand this down in the first place.
 *
 *  Throttled with the stray touch above, since a cover persists and a flick
 *  under one buys nothing a single line does not. */
let lastCoveredTouchAt = Number.NEGATIVE_INFINITY;

function noteCoveredTouch(t: ProbeTouch, rowRect: ProbeRect | null): void {
  const now = Date.now();
  if (now - lastCoveredTouchAt < STRAY_TOUCH_THROTTLE_MS) return;
  lastCoveredTouchAt = now;
  recordPress({
    face: 'the row',
    verdict: 'covered',
    movedPx: 0,
    cover: coverOverShell(),
    ...touchLanding(t, rowRect),
  });
}

/** A click with no touch behind it that reached no composer face.
 *
 *  The click-side twin of `keyboard-touch`, and the last silent path an input
 *  could take. `click-no-touch` already names a touchless click that LANDS on a
 *  face, which is the page taking clicks while the gesture pipeline is dead.
 *  One that lands anywhere else says the same about the pipeline, and it also
 *  says the page hit-tested it somewhere the composer is not.
 *
 *  Gated exactly as `noteStrayTouch` is: only while the keyboard is up, and
 *  never under a cover. Without that gate this is not the rare verdict it reads
 *  as. Two ordinary gestures produce a touchless click: a press held past the
 *  touch-behind window, and a POINTER click on a touch-capable laptop. The
 *  second writes a line per click, and a ledger whose value is that a line is
 *  unusual cannot afford that. */
let lastStrayClickAt = Number.NEGATIVE_INFINITY;

/** A click's landing, or null when it carries no coordinates.
 *
 *  A programmatic `HTMLElement.click()` dispatches with zeroes. Reading those
 *  as a point would put a hit test at the top-left corner of the screen into
 *  the line. Null says the click had no place.
 *
 *  `screenOff` rides along because a `MouseEvent` carries both spaces, and the
 *  gap between them is the one reading this module does not take from layout.
 *  A verdict about the page hit-testing elsewhere needs it most of all. */
interface ClickLanding {
  point: { x: number; y: number };
  screenOff: { x: number; y: number };
}

function clickLanding(e: {
  clientX?: number; clientY?: number; screenX?: number; screenY?: number;
}): ClickLanding | null {
  const { clientX: x, clientY: y } = e;
  if (typeof x !== 'number' || typeof y !== 'number') return null;
  if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
  if (x === 0 && y === 0) return null;
  const screenX = typeof e.screenX === 'number' ? e.screenX : x;
  const screenY = typeof e.screenY === 'number' ? e.screenY : y;
  return {
    point: { x: Math.round(x), y: Math.round(y) },
    screenOff: screenOffset({ screenX, screenY, clientX: x, clientY: y }),
  };
}

function noteStrayClick(target: Element | null, at: ClickLanding | null): void {
  if (!readViewport().keyboardActive) return;
  if (coveredOnPurpose()) return;
  const now = Date.now();
  if (now - lastStrayClickAt < STRAY_TOUCH_THROTTLE_MS) return;
  lastStrayClickAt = now;
  recordPress({
    face: describe(target) ?? 'nothing',
    verdict: 'stray-click',
    movedPx: 0,
    point: at?.point,
    screenOff: at?.screenOff,
    elementAtPoint: at ? describe(document.elementFromPoint(at.point.x, at.point.y)) : null,
    rowRect: roundRect(watchableRow()?.getBoundingClientRect() ?? null),
  });
}

/** The morph node, or null in the modes that do not render it. One reader, so
 *  the selector is written once and two callers cannot ask about two nodes. */
function morphElement(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>(`${ROW_SELECTOR} .send-cancel-morph`);
}

/** The morph button as the DOM currently holds it. */
function readMorphState(el = morphElement()): MorphState {
  return morphStateOf({
    present: !!el,
    placeholder: !!el?.classList.contains('morph-placeholder'),
    disabled: !!el?.disabled,
    label: el?.getAttribute('aria-label') ?? null,
  });
}

/** The row's one live COMMIT face: the button that sends what the user typed.
 *
 *  Two faces qualify and the row renders exactly one of them. The Send morph
 *  while it is in `send` mode, and the answer Submit while a question is
 *  pending. `PromptInput` chooses between them at one JSX position, so the two
 *  are never in the document together. That is why the morph is asked about
 *  first: its ABSENCE is what says the row is in answer mode.
 *
 *  Not `computeMorphMode`, which answers `send` for a typed answer just as it
 *  does for a typed message. The mode says what the morph WOULD show, and the
 *  choice above is what decides whether it is drawn at all.
 *
 *  NOTHING else in the row, and nothing here is pressed. It is what says the
 *  composer has something to send, which is the state the recovery and the
 *  silence verdict both need. Widening it would put Apply and Discard into a
 *  ledger that is about the composer's send. */
function commitFace(): HTMLButtonElement | null {
  const morph = morphElement();
  if (morph) return readMorphState(morph) === 'send' ? morph : null;
  return document.querySelector<HTMLButtonElement>(
    `${ROW_SELECTOR} [aria-label="Submit answer"]`,
  );
}

/** The commit face as the row holds it RIGHT NOW, or null.
 *
 *  Read at the moment it is asked about, never stored. A tap that got through
 *  has already moved the row off a commit face, and a settling Submit is held
 *  disabled on purpose. */
function liveCommitFace(): HTMLButtonElement | null {
  const face = commitFace();
  if (!face || face.disabled || !face.isConnected) return null;
  return face;
}

/** The composer's action row, and the faces inside it a press may activate.
 *
 *  `.action-btn` reaches every pill the row draws, the morph included: it
 *  carries the class alongside `.send-cancel-morph`. Naming ONE face is what
 *  blinded the previous probe. It watched the morph, and the row was in answer
 *  mode, where that node is not rendered at all.
 *
 *  It deliberately reaches no `.icon-btn`, so the row's Diff and standing apply
 *  are outside the census. `underFingerReason` reads a press on either as
 *  `other-button`, the bucket it puts an icon in. Widening to them would buy a
 *  name in the log and nothing else. The wedge these checks hunt covers the
 *  whole row, so a pill still reports it. */
const ROW_SELECTOR = '.prompt-actions-row';
const FACE_SELECTOR = '.action-btn';

/** The whole composer: the textarea, the image strip and the action row.
 *
 *  What a touch has to reach to say anything about whether the composer can be
 *  pressed. Round 15's episode delivered two touches elsewhere on the page,
 *  while the composer could not be sent from. Either one used to disarm the
 *  typing-driven recovery for the rest of the episode. */
const COMPOSER_SELECTOR = '.prompt-box';

/** Why a face cannot take a press, or that it can. Two exclusions, each a press
 *  the app drops on purpose: a `morph-placeholder` is invisible and inert,
 *  holding the row's height, and a disabled face is a settling Stop or a busy
 *  Apply.
 *
 *  Placeholder is asked FIRST, because that mode renders disabled as well. The
 *  more specific of the two overlapping answers is the useful one.
 *
 *  Structural rather than a DOM node so it tests without one. */
export type FaceExclusion = 'watchable' | 'placeholder' | 'disabled';

export function faceExclusion(face: { disabled: boolean; placeholder: boolean }): FaceExclusion {
  if (face.placeholder) return 'placeholder';
  if (face.disabled) return 'disabled';
  return 'watchable';
}

/** What sat under a finger that no watchable face claimed.
 *
 *  The tenth report is why this exists. Every `missed` line in the ledger
 *  carried no face at all. So a tap on empty row space, a tap on an `.icon-btn`
 *  and a tap on an excluded action face all read alike. Only the third is the
 *  bug.
 *
 *  `actionFace` is the exclusion of an `.action-btn` whose painted box holds the
 *  point, and null when none does. */
export type UnderFinger = 'placeholder-face' | 'disabled-face' | 'other-button' | 'nothing';

export function underFingerReason(f: {
  actionFace: FaceExclusion | null;
  otherButton: boolean;
}): UnderFinger {
  if (f.actionFace === 'placeholder') return 'placeholder-face';
  if (f.actionFace === 'disabled') return 'disabled-face';
  if (f.otherButton) return 'other-button';
  return 'nothing';
}

/** How far a point falls outside a rect, per axis and SIGNED. Zero on an axis
 *  the point is already inside.
 *
 *  The scalar below cannot tell the two readings the fourteenth episode leaves
 *  open apart: a finger that landed slightly off a face, and a page hit-testing
 *  at a fixed offset from the glass. A VECTOR narrows them across an episode. A
 *  displacement tends to repeat one `dx` and `dy`, where aim scatters and
 *  changes sign.
 *
 *  It narrows, and does not settle. Aim can cluster too, and a displacement can
 *  move with the viewport. Nothing the page reads can settle it, since every
 *  coordinate here comes from one layout. ADR 0183 carries that bound, and why
 *  `screenOff` cannot serve it on iOS Safari. */
export function missVector(
  rect: ProbeRect,
  p: { x: number; y: number },
): { dx: number; dy: number } {
  const left = p.x - rect.left;
  const right = p.x - rect.right;
  const top = p.y - rect.top;
  const bottom = p.y - rect.bottom;
  return {
    dx: Math.round(left < 0 ? left : right > 0 ? right : 0),
    dy: Math.round(top < 0 ? top : bottom > 0 ? bottom : 0),
  };
}

/** How far a point falls outside a rect, in px. Zero anywhere inside it.
 *
 *  The vector's length, and the ONE definition of that scalar, so the distance
 *  a line reports and the distance a caller compares cannot drift apart. */
export function distanceOutside(rect: ProbeRect, p: { x: number; y: number }): number {
  const { dx, dy } = missVector(rect, p);
  return Math.round(Math.hypot(dx, dy));
}

/** The face the finger came closest to, and by how much it missed.
 *
 *  A `missed` line said only that the press took no face, which left two very
 *  different states reading alike: a finger just outside a live target, and a
 *  finger nowhere near one. The second is the wedge. It is what a page
 *  hit-testing somewhere other than the glass produces.
 *
 *  Returns the caller's own entry, so a line can name the face the finger was
 *  reaching for. Null for a row holding no face at all. */
export function nearestFaceMiss<T extends { name: string; rect: ProbeRect }>(
  faces: T[],
  p: { x: number; y: number },
): { face: T; px: number; dx: number; dy: number } | null {
  let best: { face: T; px: number; dx: number; dy: number } | null = null;
  for (const f of faces) {
    const { dx, dy } = missVector(f.rect, p);
    const px = distanceOutside(f.rect, p);
    if (!best || px < best.px) best = { face: f, px, dx, dy };
  }
  return best;
}

/** The fixed offset between where the finger is and where the page says it is.
 *
 *  Every other reading here comes from the layout side: a rect, a hit test, a
 *  viewport height. They therefore agree with each other. The split this
 *  chases is between layout and the GLASS, and twelve episodes of those
 *  readings agreeing have settled nothing.
 *
 *  A touch carries both. `screenX/Y` is physical and owes nothing to layout.
 *  `clientX/Y` is what the page hit-tests with. In a standalone PWA their
 *  difference is a constant, moved only by a visual-viewport scroll the line
 *  already records. A jump in it IS the fault, stated rather than inferred. */
export function screenOffset(t: {
  screenX: number; screenY: number; clientX: number; clientY: number;
}): { x: number; y: number } {
  return { x: Math.round(t.screenX - t.clientX), y: Math.round(t.screenY - t.clientY) };
}

/** The morph button's own mode, which is what "the send button" means.
 *
 *  Read from the DOM rather than stamped by `PromptInput`, so the component
 *  keeps no diagnostic surface to remove when this module goes. */
export type MorphState = 'absent' | 'placeholder' | 'disabled' | 'cancel' | 'send';

export function morphStateOf(m: {
  present: boolean;
  placeholder: boolean;
  disabled: boolean;
  label: string | null;
}): MorphState {
  if (!m.present) return 'absent';
  if (m.placeholder) return 'placeholder';
  if (m.disabled) return 'disabled';
  return m.label === 'Cancel' ? 'cancel' : 'send';
}

/** What the report calls the button. The accessible name first, since an
 *  icon-only face has no text, and the visible label otherwise. */
export function faceName(f: { ariaLabel: string | null; text: string }): string {
  const name = f.ariaLabel?.trim() || f.text.trim();
  return name.length > 0 ? name : 'A composer button';
}

function nameOf(btn: HTMLButtonElement): string {
  return faceName({ ariaLabel: btn.getAttribute('aria-label'), text: btn.textContent ?? '' });
}

/** Every action face in the row, excluded ones included. The census the
 *  `missed` line needs, since the whole question there is what was skipped. */
function allFaces(): HTMLButtonElement[] {
  return Array.from(
    document.querySelectorAll<HTMLButtonElement>(`${ROW_SELECTOR} ${FACE_SELECTOR}`),
  );
}

function exclusionOf(btn: HTMLButtonElement): FaceExclusion {
  return faceExclusion({
    disabled: btn.disabled,
    placeholder: btn.classList.contains('morph-placeholder'),
  });
}

function watchableFaces(): HTMLButtonElement[] {
  return allFaces().filter((btn) => exclusionOf(btn) === 'watchable');
}

function describe(el: Element | null): string | null {
  if (!el) return null;
  const cls = el.classList.item(0);
  return cls ? `${el.tagName.toLowerCase()}.${cls}` : el.tagName.toLowerCase();
}

function pointerEventsOf(el: Element | null): string | null {
  return el ? getComputedStyle(el).pointerEvents : null;
}

/** The composer's action row as a box, or null when none is laid out.
 *
 *  What keeps the probe off every other tap in the app. It replaced a
 *  `document.activeElement` focus gate, which excluded the very state the fault
 *  is reported in. iOS can hold the keyboard up after focus has left the
 *  textarea. The probe then said nothing about a press on a row the user could
 *  see. Where the row IS answers that without asking about focus. */
function watchableRow(): HTMLElement | null {
  const rows = document.querySelectorAll<HTMLElement>(ROW_SELECTOR);
  for (const row of rows) {
    const rect = row.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) return row;
  }
  return null;
}

/** Which cover the app has raised over the row, or the empty string for none.
 *
 *  An open overlay inerts the shell behind it, and a client refresh dims and
 *  locks the whole page until the reload lands. A face under either is
 *  unreachable BY DESIGN, so no reading taken under one means anything.
 *
 *  It answers with a NAME rather than a flag, because declining to judge is not
 *  a reason to say nothing. A press the probe refused under a cover writes a
 *  line carrying which cover refused it. */
function coverOverShell(): string {
  const root = document.documentElement;
  const open = root.hasAttribute('data-overlay-open');
  const blocked = root.hasAttribute('data-ui-blocked');
  if (open && blocked) return 'data-overlay-open data-ui-blocked';
  if (open) return 'data-overlay-open';
  if (blocked) return 'data-ui-blocked';
  return '';
}

function coveredOnPurpose(): boolean {
  return coverOverShell() !== '';
}

/** How often the composer is asked about with no gesture behind it.
 *
 *  Slow on purpose. A wedge persists, so a faster tick buys nothing, and this
 *  runs for the whole life of the page on a phone. */
const SCHEDULED_CHECK_MS = 3000;

/** How long after the last keystroke the composer counts as waiting for a press.
 *
 *  The gap the wedge lives in. A healthy send follows the last character by well
 *  under a second, so this rarely reaches the user who types and taps. One that
 *  does costs a relayout nothing paints.
 *
 *  NOT to be cut. ADR 0228 records loosening this as rejected outright by the
 *  user. It shortens the silence rather than removing it, and every page in
 *  every session pays for that. Round 20 cut it to 1000 and reverted: the
 *  scheduled phase was what made the recovery late, not this bound.
 *  `armKeystrokeNudge` answers the phase without touching the gate. */
const UNTOUCHED_QUIET_MS = 3000;

/** How long the composer keeps counting as waiting, before the user is taken to
 *  have put the phone down.
 *
 *  A COUNT was wrong here, and the state itself is the reason. Nothing the wedge
 *  allows can reset one: it delivers no touch and no click, which are the only
 *  things that reopen a quiet window. A user re-reading a draft would spend the
 *  whole budget before the first tap. The episode the recovery exists for would
 *  then find it empty. An elapsed window cannot be spent early, and it ends
 *  only where the user has plainly stopped. */
const UNTOUCHED_WINDOW_MS = 60_000;

/** Which trigger spent a recovery. See `nudgeTrigger`. */
type NudgeTrigger = 'tick' | 'keystroke';

/** How close two nudges may come before the second is taken as the same one.
 *  Only ever collapses a tick colliding with the keystroke timer. */
const NUDGE_COALESCE_MS = 250;

/** Is this nudge the same one the last trigger already spent?
 *
 *  Two triggers reach the recovery and can come due in the same millisecond. A
 *  relayout is worth nothing twice. Far under `SCHEDULED_CHECK_MS`, so the
 *  repeat rate every earlier ledger reads is untouched.
 *
 *  Pure, so it tests without a DOM, as the other decisions here do. */
export function nudgeIsTooSoon(lastAt: number | null, now: number): boolean {
  return lastAt !== null && now - lastAt < NUDGE_COALESCE_MS;
}

/** When the recovery last relaid the shell out. See `runUntouchedNudge`. */
let lastNudgeAt: number | null = null;

/** Should the recovery run with no gesture behind it at all?
 *
 *  The fourteenth PWA episode is why this exists. The page took no touch and no
 *  click for 18.5 seconds. The composer stayed visible, its Send face answered
 *  all six scheduled checks, and every keystroke landed in the box. Both of ADR
 *  0183's answers wait to be touched, so neither could fire.
 *
 *  Typing is the one channel that survives that state, so it is what arms this.
 *  The gate is the pre-send moment and nothing else: something to commit, the
 *  user typed it, and no touch has reached THE COMPOSER since.
 *
 *  The composer, rather than the page, and round 15 is why. A touch the page
 *  took elsewhere used to disarm this for the rest of an episode. That reads a
 *  live page as a live composer, which is the partition `keyboard-touch` exists
 *  to deny.
 *
 *  Deliberately NO keyboard bound. `data-keyboard-active` means a prompt
 *  textarea is focused, which typing already implies, and ADR 0183 records what
 *  that bound cost the rescue.
 *
 *  The cover bound lives at the caller, which returns under one before reaching
 *  here. One decision point per bound, as the rescue now has. */
export function shouldNudgeUntouched(f: {
  hasCommitFace: boolean;
  typedSinceComposerInput: boolean;
  msSinceKeystroke: number;
}): boolean {
  if (!f.hasCommitFace) return false;
  if (!f.typedSinceComposerInput) return false;
  return f.msSinceKeystroke >= UNTOUCHED_QUIET_MS
    && f.msSinceKeystroke <= UNTOUCHED_WINDOW_MS;
}

/** Spend the recovery on a composer nobody has been able to touch.
 *
 *  The relayout only. That was once this trigger's own rule, and it is now the
 *  module's: nothing here presses anything.
 *
 *  The line is what the next episode reads. `nudged` says whether the shell was
 *  relaid out, and the count rides on every later press as `quiet.nudges`. */
function runUntouchedNudge(face: HTMLButtonElement | null, trigger: NudgeTrigger): void {
  const typed = lastKeystrokeAt !== null
    && (lastComposerInputAt === null || lastKeystrokeAt > lastComposerInputAt);
  const now = Date.now();
  if (nudgeIsTooSoon(lastNudgeAt, now)) return;
  const run = shouldNudgeUntouched({
    hasCommitFace: !!face,
    typedSinceComposerInput: typed,
    msSinceKeystroke: lastKeystrokeAt === null ? 0 : now - lastKeystrokeAt,
  });
  if (!face || !run) return;
  lastNudgeAt = now;
  nudgesSinceInput += 1;
  nudgesSinceKeystroke += 1;
  const nudged = relayoutShell();
  recordPress({
    face: nameOf(face),
    verdict: 'untouched',
    movedPx: 0,
    scheduled: true,
    nudgeTrigger: trigger,
    nudged,
    faceRect: roundRect(face.getBoundingClientRect()),
  });
}

/** How long the page must have taken nothing for the silence to be worth a line.
 *
 *  One scheduled tick, so the line lands on the first or second check after the
 *  keyboard goes. A user who closes the keyboard and taps straight away never
 *  reaches it. */
const SILENCE_QUIET_MS = 3000;

/** Should the ledger name a silence that followed a keyboard close?
 *
 *  The verdict the probe could never write. "No line" has meant two opposite
 *  things at once: the page took no touch, and nothing was tapped. Every other
 *  reading here waits to be touched, so none of them can tell those apart.
 *
 *  The scheduled check can. It runs through the silence, and it knows when the
 *  keyboard last went. So the line says: checks are running, the keyboard has
 *  closed, and nothing has reached the document since.
 *
 *  The silence runs from whichever came LAST, the close or the final input. An
 *  input landing just after a close is exactly what the eighteenth episode had,
 *  and anchoring on the close alone would have refused it.
 *
 *  A live commit face is the one bound that keeps this rare. It says the
 *  composer has something to send, which is the state a user taps at. Without
 *  it every idle phone would write a line.
 *
 *  The cover, mobile and visibility bounds live at the caller, as they do for
 *  the typing-driven recovery. One decision point per bound. */
export function shouldReportSilence(f: {
  hasCommitFace: boolean;
  /** Since the keyboard closed. Null when no close has been seen. */
  msSinceKeyboardClose: number | null;
  /** Since the document last took a touch or a click. Null when it has taken
   *  none at all. */
  msSinceInput: number | null;
}): boolean {
  if (!f.hasCommitFace) return false;
  if (f.msSinceKeyboardClose === null) return false;
  const silence = f.msSinceInput === null
    ? f.msSinceKeyboardClose
    : Math.min(f.msSinceKeyboardClose, f.msSinceInput);
  return silence >= SILENCE_QUIET_MS;
}

/** The close a silence line has already been written for.
 *
 *  ONE line per keyboard close. The silence it describes persists, so a line
 *  per tick would bury the reading in copies of itself. The press that ends the
 *  silence carries `quiet.ms`, and the two bracket the episode. */
let silenceReportedFor: number | null = null;

function reportSilence(face: HTMLButtonElement | null): void {
  const close = keyboardCloseState();
  if (close.at === null || silenceReportedFor === close.at) return;
  const now = Date.now();
  const msSinceInput = lastInputAt === null ? null : now - lastInputAt;
  const run = shouldReportSilence({
    hasCommitFace: !!face,
    msSinceKeyboardClose: now - close.at,
    msSinceInput,
  });
  if (!face || !run) return;
  silenceReportedFor = close.at;
  recordPress({
    face: nameOf(face),
    verdict: 'silent-since-keyboard',
    movedPx: 0,
    scheduled: true,
    // Whether the close's own relayout ran. False rules the recovery out for
    // this episode, rather than scoring it.
    nudged: close.relaidOut,
    sinceKeyboardMs: Math.round(now - close.at),
    sinceInputMs: msSinceInput === null ? null : Math.round(msSinceInput),
    closePath: close.path,
    faceRect: roundRect(face.getBoundingClientRect()),
  });
}

/** Ask the row whether it can be reached, with no user input behind it.
 *
 *  This is the one reading that does not wait to be touched. Every other path
 *  in this module needs a `touchstart`, a lift or a click to arrive first, and
 *  the tenth episode delivered none of them.
 *
 *  A row with no watchable face is skipped in silence rather than reported. An
 *  empty composer is faceless all the time, so a line there would be noise. */
function runScheduledCheck(): void {
  if (!isMobile()) return;
  if (document.visibilityState && document.visibilityState !== 'visible') return;
  if (!watchableRow()) return;
  const faces = watchableFaces();
  if (faces.length === 0) return;
  checksSinceInput += 1;
  // Asked here as well as inside the check, so a stand-down is COUNTED rather
  // than merely silent. A cover holding for a whole quiet window is the reading
  // that separates our own bookkeeping from the platform.
  if (coveredOnPurpose()) { coveredSinceInput += 1; return; }
  // ONE layout read for both. They ask the same question of the same row at
  // the same instant, and each used to query the DOM for it.
  const face = liveCommitFace();
  // The reading first, then the recovery. The line then describes the state as
  // the check found it, rather than the state the relayout left behind.
  reportSilence(face);
  runUntouchedNudge(face, 'tick');
}

let keystrokeNudgeTimer: ReturnType<typeof setTimeout> | null = null;

/** Reach the pre-send moment without waiting for the next scheduled tick.
 *
 *  Round 20 is why. The check runs every `SCHEDULED_CHECK_MS`, so a wedge that
 *  opens when typing stops waited a whole tick beyond `UNTOUCHED_QUIET_MS`
 *  before the recovery ran. The episode put that at three to six seconds, and
 *  the reporter had dismissed the keyboard by hand before it arrived.
 *
 *  A debounce, restarted by every keystroke, so a sentence arms one timer
 *  rather than one per character.
 *
 *  It takes the caller bounds the scheduled check takes, and NOT its census.
 *  `quiet.checks` counts scheduled checks, and every earlier ledger is read
 *  against that number. It leaves `reportSilence` alone too, which answers a
 *  keyboard close rather than a keystroke. */
function armKeystrokeNudge(): void {
  if (keystrokeNudgeTimer !== null) clearTimeout(keystrokeNudgeTimer);
  keystrokeNudgeTimer = setTimeout(() => {
    keystrokeNudgeTimer = null;
    if (!isMobile()) return;
    if (document.visibilityState && document.visibilityState !== 'visible') return;
    if (!watchableRow() || watchableFaces().length === 0) return;
    if (coveredOnPurpose()) return;
    runUntouchedNudge(liveCommitFace(), 'keystroke');
  }, UNTOUCHED_QUIET_MS);
}

/** Test-only: forget what the recovery has spent.
 *
 *  This module installs once and is never torn down, so its nudge counters
 *  outlive a test. One case's relayout then rides onto the next case's press
 *  line, which is how a count from one describe reached another. */
export function _resetNudgeStateForTesting(): void {
  lastNudgeAt = null;
  lastKeystrokeAt = null;
  lastComposerInputAt = null;
  nudgesSinceKeystroke = 0;
  // The quiet window and its census. `quiet.nudges` is the other carrier of a
  // relayout onto a line, so a snapshot left here rides onto the next case.
  nudgesSinceInput = 0;
  quiet = null;
  lastInputAt = null;
  checksSinceInput = 0;
  coveredSinceInput = 0;
  if (keystrokeNudgeTimer !== null) { clearTimeout(keystrokeNudgeTimer); keystrokeNudgeTimer = null; }
}

/** This event's entry for one finger, or null when another finger moved. */
function touchOf(e: TouchEvent, id: number): Touch | null {
  const changed = e.changedTouches;
  if (!changed) return null;
  for (let i = 0; i < changed.length; i++) {
    if (changed[i].identifier === id) return changed[i];
  }
  return null;
}

/** The press between `touchstart` and the lift. */
interface ArmedPress {
  el: HTMLButtonElement;
  face: string;
  /** When the press began. The outcome window is measured from here, so a claim
   *  left over from an EARLIER press can never describe this one. */
  armedAt: number;
  startX: number;
  startY: number;
  movedPx: number;
  mutations: number;
  observer: MutationObserver | null;
  /** Which finger this press belongs to. A second finger's lift, cancel or
   *  travel must not settle or move somebody else's press. */
  touchId: number;
  /** Taken at touchdown, since the lift's own reading would already carry any
   *  re-sync the gesture provoked. */
  screenOff: { x: number; y: number };
  /** The most contacts the glass has held while this press was armed. A finger
   *  joining halfway through raises it, which is why it is a running maximum
   *  rather than the count at touchdown. See `pressWasAlone`. */
  fingers: number;
  faceRect: ProbeRect | null;
  rowRect: ProbeRect | null;
  /** Every other reading on this press, taken at the same instant as the rects
   *  above. See `PressContext`. */
  at: PressContext;
  /** Fires if no lift and no cancel ever arrive. Without it the probe reports a
   *  missing lift only when the NEXT touch comes, and a pipeline that stopped
   *  delivers no next touch. That is the episode being chased, so the press
   *  would sit armed for good and write no line. */
  liftDeadline: ReturnType<typeof setTimeout> | null;
}

/** The press after the lift, waiting out its grace window for a click.
 *
 *  A SET, not a slot. The previous probe held one press and dropped it at the
 *  next `touchstart`, so the first of a double tap was never reported. Tapping
 *  again is what a user does to a dead-feeling button. That made the gesture the
 *  bug provokes the gesture that erased the evidence. */
interface SettlingPress {
  el: HTMLButtonElement;
  face: string;
  armedAt: number;
  movedPx: number;
  connectedAtLift: boolean;
  rowMutations: number;
  screenOff: { x: number; y: number };
  /** See `PressFingers`. The first is carried over from the armed press, and the
   *  second is what the lift found still down. */
  fingers: number;
  fingersAtLift: number;
  faceRect: ProbeRect | null;
  rowRect: ProbeRect | null;
  at: PressContext;
  /** Who claimed the press, snapshotted right after ITS OWN lift.
   *
   *  `takePressOutcome` is one consuming slot, so reading it at the end of a
   *  600ms window let an earlier press swallow a later press's claim. Taking it
   *  a task after the lift keeps each claim with the press that earned it. */
  outcome: PressOutcome | null;
  /** The task that takes the claim above. Cleared with the press, because
   *  `takePressOutcome` CONSUMES: left to fire on a press a click already
   *  ruled, it would eat the claim of whichever press comes next. */
  outcomeTimer: ReturnType<typeof setTimeout> | null;
  graceTimer: ReturnType<typeof setTimeout> | null;
}

/** A press that reached the composer row and no face, waiting for its lift.
 *
 *  Its `missed` line is already written at touchdown. What the lift adds is the
 *  TRAVEL, which is what tells a tap that died on the row from a swipe that
 *  began there. Only the first earns a relayout. */
interface ArmedMiss {
  touchId: number;
  startX: number;
  startY: number;
  movedPx: number;
  /** See `ArmedPress.fingers`. WebKit owes no click to a shared gesture, so a
   *  press that shared the glass is not a press that died. */
  fingers: number;
  fingersAtLift: number;
}

let installed = false;

/** Install the probe. Idempotent, and mobile-only: the report is an iOS PWA one,
 *  and a desktop click path has never been in question.
 *
 *  Every listener is passive, and none calls `preventDefault` or
 *  `stopPropagation`. A diagnostic that consumes a press becomes the bug.
 *
 *  It DISPATCHES nothing either. A dead tap used to click the commit face, and
 *  a diagnostic that acts becomes a different bug: it sent a draft on a tap
 *  138 px from Send. See ADR 0225. */
export function installDeadPressProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;

  let armed: ArmedPress | null = null;
  /** The row-missed press waiting for its lift. See `ArmedMiss`. */
  let missedPress: ArmedMiss | null = null;
  const settling = new Set<SettlingPress>();
  /** When the document last saw ANY `touchstart`, wherever it landed.
   *
   *  The one reading that separates a dead touch pipeline from a dead button. A
   *  click with nothing here behind it is a page taking clicks and no touches.
   *  An iOS standalone PWA is reported to reach that state. */
  let lastTouchStartAt: number | null = null;

  /** Rule a lifted press and write its line. Called by the grace timer, and by
   *  the click handler when a click claims the press early. */
  const rule = (press: SettlingPress, clicked: boolean) => {
    if (!settling.delete(press)) return;
    if (press.graceTimer !== null) { clearTimeout(press.graceTimer); press.graceTimer = null; }
    if (press.outcomeTimer !== null) { clearTimeout(press.outcomeTimer); press.outcomeTimer = null; }
    const fingers = { fingers: press.fingers, fingersAtLift: press.fingersAtLift };
    if (clicked) {
      recordPress({
        face: press.face,
        verdict: 'clicked',
        movedPx: press.movedPx,
        ...fingers,
        rowRect: press.rowRect,
        faceRect: press.faceRect,
        screenOff: press.screenOff,
        at: press.at,
      });
      return;
    }
    const outcome = press.outcome;
    const alone = pressWasAlone(fingers);
    const report = deadPressReport({
      face: press.face,
      movedPx: press.movedPx,
      connectedAtLift: press.connectedAtLift,
      rowMutations: press.rowMutations,
      outcome,
      alone,
      // The PRESS's viewport, the same one its line carries. A toast quoting
      // the ruling's instead reports a layout the press never saw, which is
      // the split that made the served lines unreadable.
      viewport: press.at.viewport,
    });
    recordPress({
      face: press.face,
      // A shared glass is `dead` without the fault behind it: WebKit owes no
      // click to a multi-finger gesture, so nothing was lost.
      verdict: outcome ?? (alone ? 'dead' : 'multi-touch'),
      movedPx: press.movedPx,
      connectedAtLift: press.connectedAtLift,
      rowMutations: press.rowMutations,
      ...fingers,
      toasted: report !== null,
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
      at: press.at,
    });
    if (report) showToast(report, 'warning');
  };

  /** Rule a press that reached the row and no face.
   *
   *  A stationary one is the wedge the user recovers by dismissing and
   *  reopening the keyboard, and `relayoutShell` is that recovery without the
   *  keyboard. It is what makes the SECOND tap work instead of the tenth.
   *
   *  NOTHING IS PRESSED. This path used to click the commit face, and on the
   *  seventeenth report it sent a draft on a tap 138 px from Send (ADR 0225).
   *
   *  Silent, and it waits for nothing. The grace window, the claim and the
   *  stand-down line all existed to protect a COMMIT from firing on a press
   *  something else had taken. A relayout needs no such protection: it costs
   *  nothing when it was not needed, and the `missed` line at touchdown has
   *  already said the press arrived.
   *
   *  Null for a finger that travelled, which is a scroll the platform is
   *  entitled to take. Null for a press that shared the glass, on the same
   *  grounds. */
  const ruleMissedPress = (miss: ArmedMiss) => {
    if (miss.movedPx > TAP_MOVE_THRESHOLD_PX) return;
    if (!pressWasAlone(miss)) return;
    relayoutShell();
  };

  /** Give up on an armed press whose lift never came, and SAY so.
   *
   *  Skipped while another finger is still down, because a second `touchstart`
   *  during a two-finger gesture is not a lost lift. */
  const ruleArmedWithNoLift = (press: ArmedPress, toast: boolean) => {
    press.observer?.disconnect();
    if (press.liftDeadline !== null) { clearTimeout(press.liftDeadline); press.liftDeadline = null; }
    const report = noLiftReport({
      face: press.face,
      movedPx: press.movedPx,
      // The lift that would have counted the other contacts never came, so the
      // press's own running maximum is the whole reading here.
      alone: press.fingers <= 1,
      viewport: press.at.viewport,
    });
    recordPress({
      face: press.face,
      verdict: 'no-lift',
      movedPx: press.movedPx,
      connectedAtLift: press.el.isConnected,
      rowMutations: press.mutations,
      fingers: press.fingers,
      toasted: toast && report !== null,
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
      at: press.at,
    });
    if (toast && report) showToast(report, 'warning');
  };

  // Capture, so an inert or covered target still reports.
  document.addEventListener('touchstart', (e) => {
    // A second finger joining a live gesture is neither a new press nor a lost
    // lift. Leave the gesture exactly as it is: its own lift still rules it.
    // Clearing it here stranded the press with no line at all.
    //
    // The handler COUNTS it on the way past, because WebKit owes no click to a
    // multi-finger gesture. Without this the press reads as dead, which is the
    // false alarm the seventeenth report opened with.
    //
    // A missed press is guarded too. It used to be dropped outright by the
    // fall-through below, which threw its line away.
    const live = armed ?? missedPress;
    if (live && e.touches.length > 1) {
      live.fingers = Math.max(live.fingers, e.touches.length);
      return;
    }
    const previous = armed;
    armed = null;
    if (previous) ruleArmedWithNoLift(previous, true);
    // A row-missed press whose lift never came is superseded here. Its line is
    // already written, so only the recovery is dropped. A lift that never
    // arrives is not the stationary tap that earns one.
    missedPress = null;
    if (!isMobile()) return;
    lastTouchStartAt = Date.now();
    noteInput(lastTouchStartAt);
    const touch = e.changedTouches?.[0];
    if (!touch) return;
    const row = watchableRow();
    if (!row) { noteStrayTouch(touch, e.target as Element | null, null); return; }
    const rowRect = row.getBoundingClientRect();
    const target = e.target as Element | null;
    const onRow = !!target && !!target.closest(ROW_SELECTOR);
    const inRow = inside(rowRect, touch.clientX, touch.clientY);
    // The reading the typing-driven recovery arms on. A touch that reached the
    // composer says it can be reached; one that reached the page says nothing.
    // `onRow` is not asked here: the row renders inside `.prompt-box`, so the
    // selector already covers it. `inRow` is geometric and does not.
    if (inRow || !!target?.closest(COMPOSER_SELECTOR)) {
      lastComposerInputAt = lastTouchStartAt;
    }
    // A row with no watchable face used to return here, in silence. That is a
    // real state, and a tap into it is the user pressing something that cannot
    // answer. The `missed` branch below records the census instead, so the line
    // says how many faces the row held and how many were skipped.
    const every = allFaces();
    const faces = every.filter((btn) => exclusionOf(btn) === 'watchable');
    const pressed = faces.find((f) => !!target && (target === f || f.contains(target)));
    if (!pressed) {
      // Something is over the row on purpose, so no JUDGEMENT here means
      // anything. The cover answers at the composer's own pixels, so the
      // landing report below would name it.
      //
      // Declining to judge is not declining to speak. A touch that reached the
      // row under a cover takes the `covered` line below, which is the state
      // round 11 left indistinguishable from silence.
      const covered = coveredOnPurpose();
      // Only the composer's own row is this module's business. A
      // touch counts as the row's when it was DISPATCHED there, or when it
      // landed on the row's painted box.
      //
      // A touch that is neither still gets ONE line while the keyboard is up.
      // That is the blind spot: the composer being untappable and the page
      // taking no touch at all used to be the same silence.
      //
      // Under a cover it gets none. The user is working the overlay, and a line
      // per tap inside one is the noise round 11 stood this whole branch down
      // to avoid.
      if (!onRow && !inRow) {
        if (!covered) noteStrayTouch(touch, target, roundRect(rowRect));
        return;
      }
      // The press reached the composer and the app itself is holding a cover
      // over it. Nothing below can judge that, so say it instead.
      if (covered) { noteCoveredTouch(touch, roundRect(rowRect)); return; }
      // One layout read per watchable face. Three questions here are about the
      // same boxes: which face the finger was on, the box the line carries, and
      // the distance it missed by. Each used to re-measure them.
      const boxes = faces.map((f) => ({ el: f, name: nameOf(f), rect: roundRect(f.getBoundingClientRect()) as ProbeRect }));
      const aimedAt = boxes.find((b) => inside(b.rect, touch.clientX, touch.clientY)) ?? null;
      const at = document.elementFromPoint(touch.clientX, touch.clientY);
      // Read once: `pointerEventsOf` is a computed-style call, and the report
      // and the log line want the same answer.
      const elementAtPoint = describe(at);
      const pointerEventsAtPoint = pointerEventsOf(at);
      const report = aimedAt ? landingReport({
        face: aimedAt.name,
        point: { x: touch.clientX, y: touch.clientY },
        faceRect: aimedAt.rect,
        targetIsFace: false,
        elementAtPoint,
        pointerEventsAtPoint,
        viewport: readViewport(),
      }) : null;
      // WHY no face took it. Read from the UNFILTERED row, so an excluded
      // action face under the finger is named instead of being dropped with
      // everything else. That distinction is what the ledger never carried.
      const held = every.find((f) => inside(f.getBoundingClientRect(), touch.clientX, touch.clientY));
      const under = underFingerReason({
        actionFace: held ? exclusionOf(held) : null,
        otherButton: !!at && at.closest('button') !== null,
      });
      const point = { x: Math.round(touch.clientX), y: Math.round(touch.clientY) };
      // Measured against the WATCHABLE faces: a press the row could have taken
      // is the question, and an inert placeholder answers it wrongly.
      const nearest = nearestFaceMiss(boxes, point);
      // One reading for this press, shared with whatever the lift writes about
      // it. See `PressContext`.
      const pressAt = pressContext();
      recordPress({
        face: aimedAt ? aimedAt.name : 'the row',
        verdict: 'missed',
        movedPx: 0,
        elementAtPoint,
        pointerEventsAtPoint,
        under,
        underFace: held ? nameOf(held) : null,
        faceCount: every.length,
        watchableCount: faces.length,
        rowRect: roundRect(rowRect),
        faceRect: roundRect(aimedAt?.rect ?? null),
        point,
        missedBy: missedByOf(nearest),
        screenOff: screenOffset(touch),
        fingers: e.touches.length,
        at: pressAt,
      });
      // Rule it at the lift, where the travel is known. The user recovers this
      // state by hand, and the lift runs the same relayout for them.
      //
      // `nothing` under the finger is the whole trigger. An `other-button` tap
      // ran an icon button's action, and an excluded face is a press the app
      // drops on purpose. Only a tap that reached NOTHING is a dead composer.
      missedPress = under !== 'nothing' ? null : {
        touchId: touch.identifier,
        startX: touch.screenX,
        startY: touch.screenY,
        movedPx: 0,
        fingers: e.touches.length,
        fingersAtLift: 0,
      };
      if (report) showToast(report, 'warning');
      return;
    }
    const press: ArmedPress = {
      el: pressed,
      face: nameOf(pressed),
      armedAt: Date.now(),
      startX: touch.screenX,
      startY: touch.screenY,
      movedPx: 0,
      mutations: 0,
      observer: null,
      touchId: touch.identifier,
      screenOff: screenOffset(touch),
      fingers: e.touches.length,
      faceRect: roundRect(pressed.getBoundingClientRect()),
      rowRect: roundRect(rowRect),
      at: pressContext(),
      liftDeadline: null,
    };
    press.liftDeadline = setTimeout(() => {
      press.liftDeadline = null;
      if (armed !== press) return;
      armed = null;
      // The LOG only. The finger may still be down, so all this knows is that
      // the lift is overdue, not that the press died. A toast asserting it died
      // would contradict the send a late lift still runs. The user's next tap
      // takes the path above, which does toast.
      ruleArmedWithNoLift(press, false);
    }, LIFT_DEADLINE_MS);
    // Watch the row, not the page: whether the composer rebuilds its own buttons
    // mid-press is the question, and a page-wide observer would answer a
    // different one at a much higher cost. The PRESSED face's own row, which
    // need not be the one `watchableRow` picked.
    const pressedRow = pressed.closest(ROW_SELECTOR);
    if (pressedRow) {
      press.observer = new MutationObserver((records) => { press.mutations += records.length; });
      press.observer.observe(pressedRow, {
        childList: true,
        subtree: true,
        characterData: true,
        attributes: true,
      });
    }
    armed = press;
  }, { capture: true, passive: true });

  document.addEventListener('touchmove', (e) => {
    // Screen coordinates, for the reason `TapPointer` records: with the keyboard
    // up the visual viewport settles under a stationary finger, so client ones
    // report travel that never happened.
    const travel = (
      p: { touchId: number; startX: number; startY: number; movedPx: number },
    ) => {
      const touch = touchOf(e, p.touchId);
      if (!touch) return;
      p.movedPx = Math.max(
        p.movedPx,
        Math.abs(touch.screenX - p.startX),
        Math.abs(touch.screenY - p.startY),
      );
    };
    if (armed) travel(armed);
    if (missedPress) travel(missedPress);
  }, { capture: true, passive: true });

  // CAPTURE phase. It used to bubble, and read `defaultPrevented` as proof a
  // path had worked. Both halves of that were wrong, and each hid an episode.
  //
  // A bubble listener on `document` never runs once anything upstream has
  // called `stopPropagation` in the capture phase. The dispatch checks that
  // flag before invoking each object in the path. The overlay contract's
  // paired swallow calls it, so the probe went silent on the one press shape
  // it most needed to see.
  //
  // And the touch path cancels the default BEFORE running its action, so
  // `defaultPrevented` never distinguished a press that ran from one that was
  // eaten. Both now say which they were, through `takePressOutcome`.
  document.addEventListener('touchend', (e) => {
    const miss = missedPress;
    if (miss && touchOf(e, miss.touchId)) {
      missedPress = null;
      // What is still down once this contact has gone. Zero says the gesture
      // was this finger alone. See `pressWasAlone`.
      miss.fingersAtLift = e.touches.length;
      ruleMissedPress(miss);
    }
    const press = armed;
    if (!press || !touchOf(e, press.touchId)) return;
    armed = null;
    press.observer?.disconnect();
    press.observer = null;
    if (press.liftDeadline !== null) { clearTimeout(press.liftDeadline); press.liftDeadline = null; }
    const lifted: SettlingPress = {
      el: press.el,
      face: press.face,
      armedAt: press.armedAt,
      movedPx: press.movedPx,
      connectedAtLift: press.el.isConnected,
      rowMutations: press.mutations,
      screenOff: press.screenOff,
      fingers: press.fingers,
      fingersAtLift: e.touches.length,
      faceRect: press.faceRect,
      rowRect: press.rowRect,
      at: press.at,
      outcome: null,
      outcomeTimer: null,
      graceTimer: null,
    };
    settling.add(lifted);
    // A task later, so the claim is this press's own. Both claimants run after
    // this capture listener: the touch path's `notePressOutcome` bubbles to the
    // button, and the overlay's paired swallow is a later capture registration.
    // Both have run by the time a task does.
    lifted.outcomeTimer = setTimeout(() => {
      lifted.outcomeTimer = null;
      lifted.outcome = takePressOutcome(Date.now() - lifted.armedAt);
    }, 0);
    // A click may still be coming, so give it the grace window before calling
    // the press dead.
    lifted.graceTimer = setTimeout(() => {
      lifted.graceTimer = null;
      rule(lifted, false);
    }, CLICK_GRACE_MS);
  }, { capture: true, passive: true });

  document.addEventListener('touchcancel', (e) => {
    // A gesture the system took is the platform working, so the row-missed
    // press goes with it and earns no recovery.
    if (missedPress && touchOf(e, missedPress.touchId)) missedPress = null;
    const press = armed;
    if (!press || !touchOf(e, press.touchId)) return;
    armed = null;
    press.observer?.disconnect();
    if (press.liftDeadline !== null) { clearTimeout(press.liftDeadline); press.liftDeadline = null; }
    const report = canceledPressReport({
      face: press.face,
      movedPx: press.movedPx,
      alone: pressWasAlone({ fingers: press.fingers, fingersAtLift: e.touches.length }),
      viewport: press.at.viewport,
    });
    recordPress({
      face: press.face,
      verdict: 'canceled',
      movedPx: press.movedPx,
      fingers: press.fingers,
      fingersAtLift: e.touches.length,
      toasted: report !== null,
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
      at: press.at,
    });
    if (report) showToast(report, 'warning');
  }, { capture: true, passive: true });

  // Only the PRESSED face settles its own press. Settling on any click let one
  // landing elsewhere cancel a real report, and a neighbouring face answering
  // says nothing about this one.
  //
  // The exception is the face Preact replaced under the finger: its successor is
  // a different node in the same row, and a click reaching that IS this press
  // being served. `isConnected` is what tells the two cases apart.
  //
  // A click matching NO press is the other half of this round. With no
  // `touchstart` behind it, the page is taking clicks while the touch pipeline
  // is dead. iOS standalone PWAs are reported to reach exactly that.
  document.addEventListener('click', (e) => {
    const target = e.target as Element | null;
    if (!target) return;
    // A click on the composer arms nothing. It does say the composer can be
    // reached, which is what the typing-driven recovery waits to stop.
    if (target.closest(COMPOSER_SELECTOR)) lastComposerInputAt = Date.now();
    // A TOUCHLESS click opens a fresh quiet window, and a paired one must not.
    // The synthetic click lands about 50ms after its own `touchstart`, and a
    // press records 600ms later still. So resetting here would hand the press
    // that ENDED a silence a 50ms window, losing the reading entirely.
    const touchBehind = lastTouchStartAt !== null
      && Date.now() - lastTouchStartAt < TOUCH_BEHIND_CLICK_MS;
    if (!touchBehind) noteInput(Date.now());
    // Newest first. Two taps on one face can settle at once, and the click
    // belongs to the later of them. Insertion order handed it to the older
    // press, which reversed the evidence: the tap that died read `clicked` and
    // the retry that worked read `dead`.
    for (const press of Array.from(settling).reverse()) {
      const onPressedFace = press.el.contains(target);
      const onReplacement = !press.el.isConnected && !!target.closest(ROW_SELECTOR);
      if (!onPressedFace && !onReplacement) continue;
      rule(press, true);
      return;
    }
    // A device that cannot produce a touch cannot have a touch pipeline that
    // stopped. `isMobile` is a viewport width, so a narrow desktop window would
    // otherwise log every composer click as the very split being chased.
    if (!isMobile() || !isTouchDevice()) return;
    if (touchBehind) return;
    const face = watchableFaces().find((f) => target === f || f.contains(target));
    // A touchless click that reached no face used to return in silence, which
    // is the last of the three silences an input could disappear into. It says
    // the same thing about the pipeline and more about the hit test.
    if (!face) {
      noteStrayClick(target, clickLanding(e));
      return;
    }
    // No toast. The click RAN the button's action, so the user got what they
    // asked for. What the line records is that they got it through the path
    // that was still alive.
    recordPress({
      face: nameOf(face),
      verdict: 'click-no-touch',
      movedPx: 0,
      rowRect: roundRect(watchableRow()?.getBoundingClientRect() ?? null),
      faceRect: roundRect(face.getBoundingClientRect()),
    });
  }, { capture: true, passive: true });

  // The channel that survives the wedge. Everything above waits for a touch or
  // a click, and the fourteenth PWA episode delivered neither for 18.5 seconds
  // while every keystroke reached the box.
  //
  // Deliberately NOT `noteInput`. The quiet window is a reading about touches
  // and clicks, and resetting it here would erase the silence it brackets.
  document.addEventListener('input', (e) => {
    const el = e.target as HTMLElement | null;
    if (el?.dataset?.role !== 'prompt-input') return;
    lastKeystrokeAt = Date.now();
    // A fresh keystroke opens a fresh pre-send moment, so the relayouts spent
    // in the previous one belong to it and not to this. Deliberately NOT tied
    // to `noteInput`, whose window this must outlive.
    nudgesSinceKeystroke = 0;
    armKeystrokeNudge();
  }, { capture: true, passive: true });

  // The one reading that needs no gesture. Everything above waits to be
  // touched, and a page that cannot be touched never reaches any of it.
  setInterval(runScheduledCheck, SCHEDULED_CHECK_MS);
}
