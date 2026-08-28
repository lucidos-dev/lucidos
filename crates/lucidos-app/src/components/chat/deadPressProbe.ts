import { showToast } from '../../store/store';
import { isMobile } from '../../utils/viewport';
import { TAP_MOVE_THRESHOLD_PX, takePressOutcome, type PressOutcome } from '../../utils/tapGesture';
import { postClientLog } from '../../utils/clientLog';

/** Reports a tap on a composer action button that produced nothing.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1 and removed once
 *  a report names the cause. The bug behind it has been reported six times and
 *  nobody can reproduce it: it strikes now and then on an iOS PWA and kills the
 *  composer's buttons wherever the finger presses. No emulator reproduces it, so
 *  the app has to be the one that says what happened.
 *
 *  Five questions, and the answers point at different halves of the stack.
 *
 *  Is the face reachable where it is drawn? That one is put to the face's own
 *  centre, so the finger plays no part in it. See `faceHitTestReport`.
 *
 *  Was the button still in the document when the finger lifted? A `touchend`
 *  goes to the element the press STARTED on, so a replaced node takes the touch
 *  path down with the click path. Apple's Handling Events page is explicit that
 *  a page change during the tap cascade stops the rest of it, `click` included.
 *  That answer is decisive, so it is asked first.
 *
 *  Did the press even reach the button? If not, paint and hit-testing disagree,
 *  and the viewport numbers say by how much. Did the system take the gesture,
 *  leaving no path to run? Or did the press arrive intact and both paths decline,
 *  which is code we own?
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
 *  both readings come from the browser at the same instant. */
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

/** How long after `touchend` a `click` counts as belonging to that press.
 *  Longer than WebKit's synthetic-click delay, short enough that the toast still
 *  belongs to the tap the user just made. */
const CLICK_GRACE_MS = 600;

/** What the press ended as. `served` and `swallowed` come from whoever took it;
 *  the other three are the probe's own readings. */
type PressVerdict = PressOutcome | 'dead' | 'clicked' | 'canceled' | 'missed';

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
  unreachable?: boolean;
  toasted?: boolean;
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
function firstUnreachableFace(faces: HTMLButtonElement[]): string | null {
  if (document.documentElement.hasAttribute('data-overlay-open')) return null;
  let fresh: string | null = null;
  for (const face of faces) {
    const rect = face.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) continue;
    const centre = { x: (rect.left + rect.right) / 2, y: (rect.top + rect.bottom) / 2 };
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
    fresh ??= report;
  }
  return fresh;
}

/** The in-flight press. Held from `touchstart` until something activates or the
 *  grace window closes. */
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
  let graceTimer: ReturnType<typeof setTimeout> | null = null;

  /** Drop the in-flight press without ruling on it. */
  const settle = () => {
    armed?.observer?.disconnect();
    armed = null;
    if (graceTimer !== null) { clearTimeout(graceTimer); graceTimer = null; }
  };

  // Capture, so an inert or covered target still reports.
  document.addEventListener('touchstart', (e) => {
    settle();
    if (!isMobile()) return;
    const touch = e.changedTouches?.[0];
    if (!touch) return;
    const row = watchableRow();
    if (!row) return;
    // The cheap bail, before anything that costs a style read. Only the
    // composer's own row is this module's business, and every other touch in
    // the app arrives here now that the focus gate is gone. A touch counts as
    // the row's when it was DISPATCHED there, or when it landed on the row's
    // painted box: the second is the hit-test mismatch this chases, where the
    // two disagree.
    const target = e.target as Element | null;
    const onRow = !!target && !!target.closest(ROW_SELECTOR);
    const inRow = inside(row.getBoundingClientRect(), touch.clientX, touch.clientY);
    if (!onRow && !inRow) return;
    const faces = watchableFaces();
    if (faces.length === 0) return;
    const pressed = faces.find((f) => !!target && (target === f || f.contains(target)));
    if (!pressed) {
      // Two questions worth asking, and only of a press that missed. Is any
      // face unreachable at its own centre? And did this touch land on a
      // face's painted box yet go somewhere else?
      const unreachable = firstUnreachableFace(faces);
      if (unreachable) showToast(unreachable, 'warning');
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
        unreachable: unreachable !== null,
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
    };
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
    const touch = e.changedTouches?.[0];
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
  // eaten. Both now say which they were, through `takePressOutcome`. It is
  // read in the grace callback, because the swallow runs after this listener.
  document.addEventListener('touchend', () => {
    const press = armed;
    if (!press) return;
    // Read both BEFORE the grace window. They describe this press, and a
    // re-render arriving later would answer about something else.
    const connectedAtLift = press.el.isConnected;
    const rowMutations = press.mutations;
    press.observer?.disconnect();
    press.observer = null;
    // A click may still be coming, so give it the grace window before calling
    // the press dead.
    graceTimer = setTimeout(() => {
      graceTimer = null;
      if (armed !== press) return;
      armed = null;
      // Measured from the ARM, not from a constant: a claim recorded before
      // this press began belongs to an earlier one, and reading it here would
      // call a genuinely dead press served.
      const outcome = takePressOutcome(Date.now() - press.armedAt);
      // `movedPx` is read here rather than at the lift, so the whole gesture is
      // measured. The report is null for a press that travelled, and for one
      // somebody claimed.
      const report = deadPressReport({
        face: press.face,
        movedPx: press.movedPx,
        connectedAtLift,
        rowMutations,
        outcome,
        viewport: readViewport(),
      });
      recordPress({
        face: press.face,
        verdict: outcome ?? 'dead',
        movedPx: press.movedPx,
        connectedAtLift,
        rowMutations,
        toasted: report !== null,
      });
      if (report) showToast(report, 'warning');
    }, CLICK_GRACE_MS);
  }, { capture: true, passive: true });

  document.addEventListener('touchcancel', () => {
    if (!armed) return;
    const report = canceledPressReport({
      face: armed.face,
      movedPx: armed.movedPx,
      viewport: readViewport(),
    });
    recordPress({
      face: armed.face,
      verdict: 'canceled',
      movedPx: armed.movedPx,
      toasted: report !== null,
    });
    settle();
    if (report) showToast(report, 'warning');
  }, { capture: true, passive: true });

  // Only the PRESSED face settles its own press. Settling on any click let one
  // landing elsewhere cancel a real report, and a neighbouring face answering
  // says nothing about this one.
  //
  // The exception is the face Preact replaced under the finger: its successor is
  // a different node in the same row, and a click reaching that IS this press
  // being served. `isConnected` is what tells the two cases apart.
  document.addEventListener('click', (e) => {
    const press = armed;
    if (!press) return;
    const target = e.target as Element | null;
    if (!target) return;
    const onPressedFace = press.el.contains(target);
    const onReplacement = !press.el.isConnected && !!target.closest(ROW_SELECTOR);
    if (!onPressedFace && !onReplacement) return;
    recordPress({ face: press.face, verdict: 'clicked', movedPx: press.movedPx });
    settle();
  }, { capture: true, passive: true });
}
