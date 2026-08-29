import { showToast } from '../../store/store';
import { isMobile, isTouchDevice } from '../../utils/viewport';
import { TAP_MOVE_THRESHOLD_PX, takePressOutcome, type PressOutcome } from '../../utils/tapGesture';
import { postClientLog } from '../../utils/clientLog';

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

/** The state that separates a layout fault from an event fault. Carried in every
 *  report, because the reporter is a phone with a screenshot.
 *
 *  `keyboardActive` is the `data-keyboard-active` flag on `<html>`. A whole block
 *  of `styles/mobile.css` inerts the header, the title row, the edge-swipe zones
 *  and the transcript's children off it, and re-enables `.prompt-area`. A flag
 *  out of step with the keyboard would read exactly like this bug. */
export interface ProbeViewport {
  vvHeight: number;
  vvOffsetTop: number;
  innerHeight: number;
  appHeight: string;
  keyboardActive: boolean;
  /** `window.scrollY`. A LAYOUT viewport scrolled under a fixed shell is the
   *  textbook form of this bug on iOS, and no report has carried the number. */
  pageScrollY: number;
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
  | 'unreachable';

/** How often the reachability question may be asked.
 *
 *  It costs a hit test and a style read per face, and it is now asked for
 *  touches that never reach the composer. A wedge persists, so asking on every
 *  touch buys nothing that the user's second tap does not. */
const REACHABILITY_THROTTLE_MS = 400;

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
}): void {
  postClientLog('composer-press', `${facts.face}: ${facts.verdict}`, {
    ...facts,
    movedPx: Math.round(facts.movedPx),
    viewport: readViewport(),
  });
}

/** The composer's action row, and the faces inside it a press may activate.
 *
 *  `.action-btn` reaches all of them, the morph included: it carries the class
 *  alongside `.send-cancel-morph`. Naming ONE face is what blinded the previous
 *  probe. It watched the morph, and the row was in answer mode, where that node
 *  is not rendered at all. */
const ROW_SELECTOR = '.prompt-actions-row';
const FACE_SELECTOR = '.action-btn';

/** A face a press is entitled to activate. Two exclusions, each a press the app
 *  drops on purpose: a `morph-placeholder` is invisible and inert, holding the
 *  row's height, and a disabled face is a settling Stop or a busy Apply.
 *
 *  Structural rather than a DOM node so it tests without one. */
export function pressIsWatchable(face: { disabled: boolean; placeholder: boolean }): boolean {
  return !face.disabled && !face.placeholder;
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

function watchableFaces(): HTMLButtonElement[] {
  const faces = document.querySelectorAll<HTMLButtonElement>(`${ROW_SELECTOR} ${FACE_SELECTOR}`);
  return Array.from(faces).filter((btn) => pressIsWatchable({
    disabled: btn.disabled,
    placeholder: btn.classList.contains('morph-placeholder'),
  }));
}

function readViewport(): ProbeViewport {
  const vv = window.visualViewport;
  return {
    vvHeight: vv?.height ?? window.innerHeight,
    vvOffsetTop: vv?.offsetTop ?? 0,
    innerHeight: window.innerHeight,
    appHeight: document.documentElement.style.getPropertyValue('--app-height'),
    keyboardActive: document.documentElement.hasAttribute('data-keyboard-active'),
    pageScrollY: window.scrollY,
  };
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

/** The first watchable face the browser does not answer with at its own centre,
 *  as a ready report. Null when every face is reachable, which is the healthy
 *  case and the usual one.
 *
 *  Silent while an overlay is open: that is the one time something is MEANT to
 *  cover the row, and the app inerts the shell behind it on purpose. Silent too
 *  for a face with no box, which is a row mid-layout rather than a fault. */
function firstUnreachableFace(
  faces: HTMLButtonElement[],
): { face: string; report: string; rect: ProbeRect } | null {
  if (document.documentElement.hasAttribute('data-overlay-open')) return null;
  let fresh: { face: string; report: string; rect: ProbeRect } | null = null;
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
    if (!report) { reportedUnreachable.delete(name); continue; }
    if (reportedUnreachable.has(name)) continue;
    reportedUnreachable.add(name);
    fresh ??= { face: name, report, rect: roundRect(rect) as ProbeRect };
  }
  return fresh;
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
    });
    if (report) showToast(report, 'warning');
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
    if (!isMobile()) return;
    lastTouchStartAt = Date.now();
    const touch = e.changedTouches?.[0];
    if (!touch) return;
    const row = watchableRow();
    if (!row) return;
    const rowRect = row.getBoundingClientRect();
    const target = e.target as Element | null;
    const onRow = !!target && !!target.closest(ROW_SELECTOR);
    const inRow = inside(rowRect, touch.clientX, touch.clientY);
    const faces = watchableFaces();
    if (faces.length === 0) return;
    const pressed = faces.find((f) => !!target && (target === f || f.contains(target)));
    if (!pressed) {
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
        }
      }
      // Past that, only the composer's own row is this module's business. A
      // touch counts as the row's when it was DISPATCHED there, or when it
      // landed on the row's painted box.
      if (!onRow && !inRow) return;
      const aimedAt = faces.find((f) => inside(f.getBoundingClientRect(), touch.clientX, touch.clientY));
      const at = document.elementFromPoint(touch.clientX, touch.clientY);
      // Read once: `pointerEventsOf` is a computed-style call, and the report
      // and the log line want the same answer.
      const elementAtPoint = describe(at);
      const pointerEventsAtPoint = pointerEventsOf(at);
      const report = aimedAt ? landingReport({
        face: nameOf(aimedAt),
        point: { x: touch.clientX, y: touch.clientY },
        faceRect: aimedAt.getBoundingClientRect(),
        targetIsFace: false,
        elementAtPoint,
        pointerEventsAtPoint,
        viewport: readViewport(),
      }) : null;
      recordPress({
        face: aimedAt ? nameOf(aimedAt) : 'the row',
        verdict: 'missed',
        movedPx: 0,
        elementAtPoint,
        pointerEventsAtPoint,
        rowRect: roundRect(rowRect),
        faceRect: roundRect(aimedAt?.getBoundingClientRect() ?? null),
      });
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
    if (!armed) return;
    const touch = touchOf(e, armed.touchId);
    if (!touch) return;
    // Screen coordinates, for the reason `TapPointer` records: with the keyboard
    // up the visual viewport settles under a stationary finger, so client ones
    // report travel that never happened.
    armed.movedPx = Math.max(
      armed.movedPx,
      Math.abs(touch.screenX - armed.startX),
      Math.abs(touch.screenY - armed.startY),
    );
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
    const touchBehind = lastTouchStartAt !== null
      && Date.now() - lastTouchStartAt < TOUCH_BEHIND_CLICK_MS;
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
}
