import { showToast } from '../../store/store';
import { isMobile, isTouchDevice } from '../../utils/viewport';
import { TAP_MOVE_THRESHOLD_PX, takePressOutcome, type PressOutcome } from '../../utils/tapGesture';
import { postClientLog } from '../../utils/clientLog';
import { readViewport, type ProbeViewport } from './probeViewport';

// Re-exported so this module stays the one import for a press report's types.
// The reading itself is shared with the keystroke probe, which is why it moved.
export type { ProbeViewport };

/** Reports a tap on a composer action button that produced nothing.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1 and removed once
 *  a report names the cause. The bug behind it has been reported eight times and
 *  nobody can reproduce it: it strikes now and then on an iOS PWA and kills the
 *  composer's buttons wherever the finger presses. No emulator reproduces it, so
 *  the app has to be the one that says what happened.
 *
 *  The eighth episode was SILENT, and that is what this round is shaped by. The
 *  probe used to arm only from a `touchstart` it could attribute to the row. A
 *  gesture the page never received therefore left no trace at all. It now
 *  partitions the four ways a press can go missing, and the plan behind that
 *  partition is
 *  `docs/plans/2026-08-29-the-composer-says-when-send-is-unreachable.md`.
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

/** Is the button where the browser thinks it is? Asked of the face's OWN centre,
 *  so the finger plays no part in the answer.
 *
 *  The landing check above needs the touch point to fall inside a painted rect.
 *  A coordinate space out of step with layout misses every rect, and the probe
 *  then said nothing about the very state it was built for. This one is immune:
 *  it compares where the row is PAINTED with what the page answers there, and
 *  both readings come from the browser at the same instant.
 *
 *  Immune, but for two rounds it was asked only AFTER a gate that the same fault
 *  defeats. `installDeadPressProbe` now asks it in front of that gate. */
export interface FaceHitTestFacts {
  face: string;
  centre: { x: number; y: number };
  /** `elementFromPoint` at the centre answered with the face, or something
   *  inside it. Anything else means the face is not reachable where it is drawn:
   *  an ancestor means the face itself takes no pointer, and an unrelated
   *  element means something is over it or the hit-test tree is stale. */
  answeredWithFace: boolean;
  elementAtCentre: string | null;
  pointerEventsAtCentre: string | null;
  viewport: ProbeViewport;
}

export function faceHitTestReport(f: FaceHitTestFacts): string | null {
  if (f.answeredWithFace) return null;
  return `${f.face} is not reachable where it is drawn: at its own centre `
    + `(${Math.round(f.centre.x)}, ${Math.round(f.centre.y)}) the page answers `
    + `${f.elementAtCentre ?? 'nothing'} `
    + `(pointer-events ${f.pointerEventsAtCentre ?? 'unknown'}). `
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
 *  would name its own cause. */
export function deadPressReport(f: DeadPressFacts): string | null {
  if (f.outcome !== null) return null;
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
 *  being chased. */
export function canceledPressReport(f: {
  face: string;
  movedPx: number;
  viewport: ProbeViewport;
}): string | null {
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
 *  working. */
export function noLiftReport(f: {
  face: string;
  movedPx: number;
  viewport: ProbeViewport;
}): string | null {
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
 *  no gesture behind it at all. */
type PressVerdict =
  | PressOutcome
  | 'dead'
  | 'clicked'
  | 'canceled'
  | 'missed'
  | 'no-lift'
  | 'click-no-touch'
  | 'unreachable'
  | 'repaired'
  | 'repair-failed'
  | 'activated'
  | 'keyboard-touch';

/** How often the reachability question may be asked.
 *
 *  It costs a hit test and a style read per face, and it is now asked for
 *  touches that never reach the composer. A wedge persists, so asking on every
 *  touch buys nothing that the user's second tap does not. */
const REACHABILITY_THROTTLE_MS = 400;

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
 *  Every line therefore says how long the page had been silent beforehand. It
 *  says how many scheduled checks ran in that silence, and how many found the
 *  row unreachable. Those three read together bracket an episode. Riding on
 *  lines that are written anyway costs no new noise. */
interface QuietWindow {
  ms: number;
  checks: number;
  unreachable: number;
}

let lastInputAt: number | null = null;
let checksSinceInput = 0;
let unreachableSinceInput = 0;
let quiet: QuietWindow | null = null;

/** Close the running quiet window and open a fresh one. Called for every
 *  `touchstart` and every `click` the document sees, wherever they land. */
function noteInput(now: number): void {
  if (lastInputAt !== null) {
    quiet = {
      ms: Math.round(now - lastInputAt),
      checks: checksSinceInput,
      unreachable: unreachableSinceInput,
    };
  }
  lastInputAt = now;
  checksSinceInput = 0;
  unreachableSinceInput = 0;
}

/** The engine-log breadcrumb, written for EVERY press the probe watches.
 *
 *  A toast reports to whoever is looking at the screen and keeps it. That is
 *  how five episodes produced nothing to work from. This lands in `engine.log`
 *  instead, so an episode can be read back afterwards from the workspace.
 *
 *  It carries no draft text and no message content: what the user typed has no
 *  business in a log line (`.claude/rules/no-private-data.md`). */
function recordPress(facts: {
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
  missedBy?: { face: string; px: number } | null;
  /** `screenOffset` for this press. The one reading not taken from the layout
   *  side, so a page hit-testing away from the glass says so here. */
  screenOff?: { x: number; y: number };
  /** Written with no user input behind it, by the scheduled check. */
  scheduled?: boolean;
  /** Whether the relayout actually ran. A `repair-failed` that never nudged
   *  rules nothing out, unlike one that nudged and did not help. */
  nudged?: boolean;
  /** Whether the face was still in the document when the repair was judged.
   *  A row that re-rendered answers nothing, and that is not a failure. */
  connected?: boolean;
}): void {
  postClientLog('composer-press', `${facts.face}: ${facts.verdict}`, {
    ...facts,
    movedPx: Math.round(facts.movedPx),
    // The composer's own state. "The button is there, it just doesn't work" is
    // a claim about this, and no line has ever carried it.
    morph: readMorphState(),
    quiet,
    viewport: readViewport(),
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

function noteStrayTouch(
  t: { clientX: number; clientY: number; screenX: number; screenY: number },
  target: Element | null,
  rowRect: ProbeRect | null,
): void {
  if (!readViewport().keyboardActive) return;
  const now = Date.now();
  if (now - lastStrayTouchAt < STRAY_TOUCH_THROTTLE_MS) return;
  lastStrayTouchAt = now;
  const point = { x: Math.round(t.clientX), y: Math.round(t.clientY) };
  recordPress({
    face: describe(target) ?? 'nothing',
    verdict: 'keyboard-touch',
    movedPx: 0,
    point,
    screenOff: screenOffset(t),
    elementAtPoint: describe(document.elementFromPoint(point.x, point.y)),
    rowRect,
  });
}

/** The morph button as the DOM currently holds it. */
function readMorphState(): MorphState {
  const el = document.querySelector<HTMLButtonElement>(`${ROW_SELECTOR} .send-cancel-morph`);
  return morphStateOf({
    present: !!el,
    placeholder: !!el?.classList.contains('morph-placeholder'),
    disabled: !!el?.disabled,
    label: el?.getAttribute('aria-label') ?? null,
  });
}

/** Run Send for a press the page dropped, and say whether it did.
 *
 *  The app knows enough to do this. A touch reached the document, its
 *  coordinates were inside the composer row, no button claimed it, and Send is
 *  live with a draft behind it. Relaying out the shell helps the NEXT tap. This
 *  is what answers the one the user just made (ADR 0183).
 *
 *  Four bounds, and each is a state where the intent is not certain. Nothing
 *  may be covering the composer. SEND mode only, so a dropped tap can never
 *  stop a running turn. The keyboard must be UP, which is the state every
 *  report describes. And the morph is re-read at the moment of firing. A
 *  second tap that got through has already moved it off `send`. */
function rescueSend(): boolean {
  // A cover can go up in the grace window between the tap and this, and a
  // synthetic click ignores it. Under one, the composer is unreachable by
  // design and the user is looking at something else.
  if (coveredOnPurpose()) return false;
  if (readMorphState() !== 'send') return false;
  if (!readViewport().keyboardActive) return false;
  const el = document.querySelector<HTMLButtonElement>(`${ROW_SELECTOR} .send-cancel-morph`);
  if (!el || el.disabled || !el.isConnected) return false;
  // The morph's click path asks its tap gate, and a gate holding no press
  // counts as a tap: that is how a keyboard Enter activates. See `createTapGate`.
  el.click();
  return true;
}

/** The composer's action row, and the faces inside it a press may activate.
 *
 *  `.action-btn` reaches all of them, the morph included: it carries the class
 *  alongside `.send-cancel-morph`. Naming ONE face is what blinded the previous
 *  probe. It watched the morph, and the row was in answer mode, where that node
 *  is not rendered at all. */
const ROW_SELECTOR = '.prompt-actions-row';
const FACE_SELECTOR = '.action-btn';

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

/** A face a press is entitled to activate. */
export function pressIsWatchable(face: { disabled: boolean; placeholder: boolean }): boolean {
  return faceExclusion(face) === 'watchable';
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

/** How far a point falls outside a rect, in px. Zero anywhere inside it. */
export function distanceOutside(rect: ProbeRect, p: { x: number; y: number }): number {
  const dx = Math.max(rect.left - p.x, 0, p.x - rect.right);
  const dy = Math.max(rect.top - p.y, 0, p.y - rect.bottom);
  return Math.round(Math.hypot(dx, dy));
}

/** The face the finger came closest to, and by how much it missed.
 *
 *  A `missed` line said only that the press took no face, which left two very
 *  different states reading alike: a finger just outside a live target, and a
 *  finger nowhere near one. The second is the wedge. It is what a page
 *  hit-testing somewhere other than the glass produces.
 *
 *  Returns the caller's own entry, so the lift can repair the face the finger
 *  was reaching for. Null for a row holding no face at all. */
export function nearestFaceMiss<T extends { name: string; rect: ProbeRect }>(
  faces: T[],
  p: { x: number; y: number },
): { face: T; px: number } | null {
  let best: { face: T; px: number } | null = null;
  for (const f of faces) {
    const px = distanceOutside(f.rect, p);
    if (!best || px < best.px) best = { face: f, px };
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

/** Can the document hit-test this point at all? `elementFromPoint` answers null
 *  outside the viewport, which is indistinguishable from a covered element. */
function onScreen(p: { x: number; y: number }): boolean {
  const vv = window.visualViewport;
  const height = vv?.height ?? window.innerHeight;
  const width = vv?.width ?? window.innerWidth;
  return p.x >= 0 && p.x <= width && p.y >= 0 && p.y <= height;
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

/** Faces already reported unreachable. This check runs on EVERY touch while the
 *  composer is focused, and its toast holds until dismissed. Without the latch,
 *  one wedged state buries the screen in copies of itself. A face is forgotten
 *  the moment it answers again, so a state that returns reports again. */
const reportedUnreachable = new Set<string>();

/** Is something MEANT to be over the row? The probe answers no question through
 *  a cover the app raised on purpose.
 *
 *  An open overlay inerts the shell behind it, and a client refresh dims and
 *  locks the whole page until the reload lands. A face under either is
 *  unreachable by design. Reporting it is a false alarm, and the episode's one
 *  repair goes on a layout nobody is waiting for.
 *
 *  The refresh half was missing, so a user got the wedge report naming
 *  `div.ui-blocking-overlay`, stacked over the app's own "Refreshing" status. */
function coveredOnPurpose(): boolean {
  const root = document.documentElement;
  return root.hasAttribute('data-overlay-open') || root.hasAttribute('data-ui-blocked');
}

/** The first watchable face the browser does not answer with at its own centre,
 *  as a ready report. Null when every face is reachable, which is the healthy
 *  case and the usual one.
 *
 *  Silent under a cover the app raised itself, which `coveredOnPurpose` names.
 *  Silent too for a face with no box, which is a row mid-layout rather than a
 *  fault. */
interface RepairTarget {
  face: string;
  el: HTMLButtonElement;
  rect: ProbeRect;
}

interface UnreachableFace extends RepairTarget {
  report: string;
}

function firstUnreachableFace(faces: HTMLButtonElement[]): UnreachableFace | null {
  if (coveredOnPurpose()) return null;
  let fresh: UnreachableFace | null = null;
  for (const face of faces) {
    const rect = face.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) continue;
    const centre = { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
    // `elementFromPoint` answers null for any point outside the viewport, so a
    // face parked off-screen would read as unreachable on every touch. The
    // mobile swipe track is 300% wide and keeps all three panes laid out. The
    // composer therefore sits off-screen whenever the user is on another pane,
    // and a point the document cannot hit-test answers no question.
    if (!onScreen(centre)) continue;
    const at = document.elementFromPoint(centre.x, centre.y);
    const name = nameOf(face);
    const report = faceHitTestReport({
      face: name,
      centre,
      answeredWithFace: !!at && face.contains(at),
      elementAtCentre: describe(at),
      pointerEventsAtCentre: pointerEventsOf(at),
      viewport: readViewport(),
    });
    if (!report) {
      // The face answers again, so the episode is over. Both latches are
      // forgotten together, or a state that returns would go unreported and
      // unrepaired.
      reportedUnreachable.delete(name);
      repairAttempted.delete(name);
      continue;
    }
    if (reportedUnreachable.has(name)) continue;
    reportedUnreachable.add(name);
    fresh ??= { face: name, el: face, report, rect: roundRect(rect) as ProbeRect };
  }
  return fresh;
}

/** Does the page answer with this face at its own painted centre? The bare
 *  question `firstUnreachableFace` wraps, asked again after a repair with no
 *  latch and no report in the way. */
function faceAnswersAtCentre(face: HTMLButtonElement): boolean {
  const rect = face.getBoundingClientRect();
  if (rect.width === 0 || rect.height === 0) return false;
  const centre = { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
  if (!onScreen(centre)) return false;
  const at = document.elementFromPoint(centre.x, centre.y);
  return !!at && face.contains(at);
}

/** How often the row is asked whether it can still be reached.
 *
 *  Slow on purpose. The check costs a hit test and a style read per face, and a
 *  wedge persists, so a faster tick buys nothing. */
const SCHEDULED_CHECK_MS = 3000;

/** How long the repair is given before the face is asked again. Past a frame,
 *  and short enough that the answer still belongs to the nudge. */
const REPAIR_SETTLE_MS = 50;

/** Faces a repair has already been spent on, cleared the moment the face
 *  answers again. One attempt per episode: a nudge that did not work will not
 *  work on the next tick either. */
const repairAttempted = new Set<string>();

/** The height to relayout the shell at before putting it back.
 *
 *  The AMPLITUDE of the user's own recovery: the keyboard's own span, which is
 *  the layout viewport less the shell's current height. A 1px wobble relaid the
 *  same boxes out at the same size and moved nothing else.
 *
 *  DOWNWARD, though the keyboard bounce goes up, and that is deliberate.
 *  Growing the shell shrinks every scroller in it. The browser clamps their
 *  scroll offsets at that layout, and putting the height back does NOT put the
 *  offsets back. A transcript at the live edge would jump most of a screen on
 *  every dead tap. Shrinking cannot clamp anything: it only makes room, and the
 *  restore returns to geometry those offsets were already valid in.
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
 *  False when the property is not set, which is a shell the probe does not own
 *  and must not start writing. */
function nudgeLayout(): boolean {
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

/** Repair a face the page will not answer with, then say whether it worked.
 *
 *  The outcome is the point. `repaired` says a stale layout was the cause and
 *  the user has their composer back. `repair-failed` rules that out, which is
 *  the reading ten reports have not produced.
 *
 *  Only `repaired` toasts, and only where the face had actually stopped
 *  answering. The user pressed something that did nothing, and the message
 *  tells them it is worth pressing again. A failed repair changes nothing they
 *  can see or act on.
 *
 *  `announce` is false for the DEAD-TAP caller, whose face answers at its own
 *  centre throughout. The hit test cannot score that repair, so a toast there
 *  would claim a fix on every stray tap on empty row space. It runs the same
 *  recovery and says so only in the log. */
function attemptRepair(found: RepairTarget, scheduled: boolean, announce = true): void {
  if (repairAttempted.has(found.face)) return;
  repairAttempted.add(found.face);
  if (!nudgeLayout()) {
    recordPress({
      face: found.face,
      verdict: 'repair-failed',
      movedPx: 0,
      scheduled,
      nudged: false,
      faceRect: found.rect,
    });
    return;
  }
  setTimeout(() => {
    // A face the row replaced under us answers nothing, and calling that a
    // failed repair would poison the very split this exists to read. The
    // episode ended by re-render, so forget it and let the next check ask.
    if (!found.el.isConnected) {
      recordPress({
        face: found.face,
        verdict: 'repair-failed',
        movedPx: 0,
        scheduled,
        nudged: true,
        connected: false,
        faceRect: found.rect,
      });
      reportedUnreachable.delete(found.face);
      repairAttempted.delete(found.face);
      return;
    }
    const ok = faceAnswersAtCentre(found.el);
    recordPress({
      face: found.face,
      verdict: ok ? 'repaired' : 'repair-failed',
      movedPx: 0,
      scheduled,
      nudged: true,
      connected: true,
      toasted: ok && announce,
      faceRect: roundRect(found.el.getBoundingClientRect()),
    });
    if (!ok) return;
    // The episode is over, so let a later one report and repair itself.
    reportedUnreachable.delete(found.face);
    repairAttempted.delete(found.face);
    if (!announce) return;
    showToast(
      `${found.face} had stopped taking taps and has been reset. Try again.`,
      'warning',
    );
  }, REPAIR_SETTLE_MS);
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
  const unreachable = firstUnreachableFace(faces);
  if (!unreachable) return;
  unreachableSinceInput += 1;
  recordPress({
    face: unreachable.face,
    verdict: 'unreachable',
    movedPx: 0,
    scheduled: true,
    toasted: true,
    faceRect: unreachable.rect,
  });
  showToast(unreachable.report, 'warning');
  attemptRepair(unreachable, true);
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
  faceRect: ProbeRect | null;
  rowRect: ProbeRect | null;
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
  faceRect: ProbeRect | null;
  rowRect: ProbeRect | null;
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
 *  The one press shape the module never ruled. Its line asserted `movedPx: 0`
 *  instead of measuring, so a swipe beginning on the row read exactly like a
 *  tap that died there. The two want opposite answers: one is the platform
 *  working, and the other is the wedge this module exists for. */
interface ArmedMiss {
  touchId: number;
  startX: number;
  startY: number;
  movedPx: number;
  /** The face the finger came nearest to, which is the one to repair. */
  target: RepairTarget | null;
}

let installed = false;

/** Install the probe. Idempotent, and mobile-only: the report is an iOS PWA one,
 *  and a desktop click path has never been in question.
 *
 *  Every listener is passive, and none calls `preventDefault` or
 *  `stopPropagation`. A diagnostic that consumes a press becomes the bug. */
export function installDeadPressProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;

  let armed: ArmedPress | null = null;
  /** The row-missed press waiting for its lift. See `ArmedMiss`. */
  let missedPress: ArmedMiss | null = null;
  /** Its grace window, once lifted. Cancelled by anything proving the gesture
   *  was not dead after all. */
  let missSettle: ReturnType<typeof setTimeout> | null = null;
  const settling = new Set<SettlingPress>();
  /** When the document last saw ANY `touchstart`, wherever it landed.
   *
   *  The one reading that separates a dead touch pipeline from a dead button. A
   *  click with nothing here behind it is a page taking clicks and no touches.
   *  An iOS standalone PWA is reported to reach that state. */
  let lastTouchStartAt: number | null = null;
  /** When the reachability question was last asked. See its throttle. */
  let lastReachabilityAt = Number.NEGATIVE_INFINITY;

  /** Rule a lifted press and write its line. Called by the grace timer, and by
   *  the click handler when a click claims the press early. */
  const rule = (press: SettlingPress, clicked: boolean) => {
    if (!settling.delete(press)) return;
    if (press.graceTimer !== null) { clearTimeout(press.graceTimer); press.graceTimer = null; }
    if (press.outcomeTimer !== null) { clearTimeout(press.outcomeTimer); press.outcomeTimer = null; }
    if (clicked) {
      recordPress({
        face: press.face,
        verdict: 'clicked',
        movedPx: press.movedPx,
        rowRect: press.rowRect,
        faceRect: press.faceRect,
        screenOff: press.screenOff,
      });
      return;
    }
    const outcome = press.outcome;
    const report = deadPressReport({
      face: press.face,
      movedPx: press.movedPx,
      connectedAtLift: press.connectedAtLift,
      rowMutations: press.rowMutations,
      outcome,
      viewport: readViewport(),
    });
    recordPress({
      face: press.face,
      verdict: outcome ?? 'dead',
      movedPx: press.movedPx,
      connectedAtLift: press.connectedAtLift,
      rowMutations: press.rowMutations,
      toasted: report !== null,
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
    });
    if (report) showToast(report, 'warning');
  };

  /** Rule a press that reached the row and no face.
   *
   *  A stationary one is the wedge the user recovers by dismissing and
   *  reopening the keyboard, and `nudgeLayout` is that recovery without the
   *  keyboard. Running it here is what makes the SECOND tap work instead of
   *  the tenth.
   *
   *  Silent, because nothing here can score the repair. The face answers at its
   *  own centre throughout this state, so a toast would claim a fix on every
   *  stray tap on empty row space.
   *
   *  A press that travelled is a scroll or a swipe that began on the row, and
   *  the platform is entitled to drop it. */
  const ruleMissedPress = (miss: ArmedMiss) => {
    if (miss.movedPx > TAP_MOVE_THRESHOLD_PX) return;
    // The grace window first. A click still on its way means the press was not
    // dead, and running Send over it would send the draft twice.
    if (missSettle !== null) clearTimeout(missSettle);
    missSettle = setTimeout(() => {
      missSettle = null;
      if (rescueSend()) {
        recordPress({
          face: 'Send message',
          verdict: 'activated',
          movedPx: miss.movedPx,
          toasted: true,
        });
        showToast('That tap did not register, so Send was run for you.', 'warning');
      }
      // The relayout runs either way: it is what the NEXT tap needs.
      if (miss.target) attemptRepair(miss.target, false, false);
    }, CLICK_GRACE_MS);
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
      viewport: readViewport(),
    });
    recordPress({
      face: press.face,
      verdict: 'no-lift',
      movedPx: press.movedPx,
      connectedAtLift: press.el.isConnected,
      rowMutations: press.mutations,
      toasted: toast && report !== null,
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
    });
    if (toast && report) showToast(report, 'warning');
  };

  // Capture, so an inert or covered target still reports.
  document.addEventListener('touchstart', (e) => {
    // A second finger joining a live gesture is neither a new press nor a lost
    // lift. Leave the armed press exactly as it is: its own lift still rules
    // it. Clearing it here stranded the press with no line at all.
    if (armed && e.touches.length > 1) return;
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
    // A row with no watchable face used to return here, in silence. That is a
    // real state, and a tap into it is the user pressing something that cannot
    // answer. The `missed` branch below records the census instead, so the line
    // says how many faces the row held and how many were skipped.
    const every = allFaces();
    const faces = every.filter((btn) => exclusionOf(btn) === 'watchable');
    const pressed = faces.find((f) => !!target && (target === f || f.contains(target)));
    if (!pressed) {
      // Something is over the row on purpose, so no reading here means anything.
      // The cover answers at the composer's own pixels, and both reports below
      // would name it.
      if (coveredOnPurpose()) return;
      // The reachability question comes FIRST, in front of the row-attribution
      // gate below. It is the one check immune to a coordinate space out of
      // step with layout. For two rounds it sat behind the very gate such a
      // disagreement defeats, so a wedge that moved the row reported nothing.
      //
      // It carries its own line and its own latch, rather than widening the
      // gate. A wedge would otherwise put a `missed` line under every touch in
      // the app for as long as it lasted.
      const now = Date.now();
      if (now - lastReachabilityAt >= REACHABILITY_THROTTLE_MS) {
        lastReachabilityAt = now;
        const unreachable = firstUnreachableFace(faces);
        if (unreachable) {
          recordPress({
            face: unreachable.face,
            verdict: 'unreachable',
            movedPx: 0,
            toasted: true,
            rowRect: roundRect(rowRect),
            faceRect: unreachable.rect,
          });
          showToast(unreachable.report, 'warning');
          // Repair from here too. This path latches the face, so leaving it to
          // the scheduled check would strand a wedge the USER found first.
          // Tapping is how they find it.
          attemptRepair(unreachable, false);
        }
      }
      // Past that, only the composer's own row is this module's business. A
      // touch counts as the row's when it was DISPATCHED there, or when it
      // landed on the row's painted box.
      //
      // A touch that is neither still gets ONE line while the keyboard is up.
      // That is the blind spot: the composer being untappable and the page
      // taking no touch at all used to be the same silence.
      if (!onRow && !inRow) { noteStrayTouch(touch, target, roundRect(rowRect)); return; }
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
        missedBy: nearest && { face: nearest.face.name, px: nearest.px },
        screenOff: screenOffset(touch),
      });
      // Rule it at the lift, where the travel is known. The user recovers this
      // state by hand, and the lift now runs the same recovery for them.
      //
      // `nothing` under the finger is the whole trigger. An `other-button` tap
      // ran an icon button's action, and an excluded face is a press the app
      // drops on purpose. Only a tap that reached NOTHING is a dead composer.
      missedPress = under !== 'nothing' ? null : {
        touchId: touch.identifier,
        startX: touch.screenX,
        startY: touch.screenY,
        movedPx: 0,
        target: nearest && { face: nearest.face.name, el: nearest.face.el, rect: nearest.face.rect },
      };
      if (report) showToast(report, 'warning');
      return;
    }
    // The finger is on a real face now, so a rescue armed by an earlier dead
    // tap must not fire behind this press.
    if (missSettle !== null) { clearTimeout(missSettle); missSettle = null; }
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
      faceRect: roundRect(pressed.getBoundingClientRect()),
      rowRect: roundRect(rowRect),
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
      faceRect: press.faceRect,
      rowRect: press.rowRect,
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
      viewport: readViewport(),
    });
    recordPress({
      face: press.face,
      verdict: 'canceled',
      movedPx: press.movedPx,
      toasted: report !== null,
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
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
    // Any click at all means the gesture was not dead, so the rescue below
    // stands down. It runs only where nothing else answered.
    if (missSettle !== null) { clearTimeout(missSettle); missSettle = null; }
    const target = e.target as Element | null;
    if (!target) return;
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
    if (!face) return;
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

  // The one reading that needs no gesture. Everything above waits to be
  // touched, and a page that cannot be touched never reaches any of it.
  setInterval(runScheduledCheck, SCHEDULED_CHECK_MS);
}
