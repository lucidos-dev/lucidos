import { showToast } from '../../store/store';
import { isMobile, isTouchDevice } from '../../utils/viewport';
import { TAP_MOVE_THRESHOLD_PX } from '@lucidos/tap-press';
import { takePressOutcome, type PressOutcome } from '../../utils/tapGesture';
import { postClientLog } from '../../utils/clientLog';
import { LONG_PRESS_DELAY_MS } from '../../hooks/useLongPress';
import { relayoutShell, keyboardCloseState, type ClosePath } from '../layout/keyboardCloseRelayout';
import { readViewport, type ProbeViewport } from './probeViewport';
import { coverOverShell, installPressCoverWatch, takeCoverForThisPress } from './pressCover';

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

export interface ProbeRect {
  left: number;
  right: number;
  top: number;
  bottom: number;
}

export interface LandingFacts {
  face: string;
  point: { x: number; y: number };
  faceRect: ProbeRect | null;
  targetIsFace: boolean;
  dispatchedTo: string | null;
  elementAtPoint: string | null;
  pointerEventsAtPoint: string | null;
  viewport: ProbeViewport;
}

export interface PressFingers {
  fingers: number;
  fingersAtLift: number;
}

export function pressWasAlone(f: PressFingers): boolean {
  return f.fingers <= 1 && f.fingersAtLift === 0;
}

export function pressWasATap(heldMs: number): boolean {
  return heldMs <= LONG_PRESS_DELAY_MS;
}

export function unclaimedVerdict(f: { alone: boolean; heldMs: number }): 'dead' | 'multi-touch' | 'long-hold' {
  if (!f.alone) return 'multi-touch';
  if (!pressWasATap(f.heldMs)) return 'long-hold';
  return 'dead';
}

export interface DeadPressFacts {
  face: string;
  movedPx: number;
  connectedAtLift: boolean;
  rowMutations: number;
  outcome: PressOutcome | null;
  alone: boolean;
  heldMs: number;
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

export function landingReport(f: LandingFacts): string | null {
  if (!f.faceRect || f.targetIsFace) return null;
  if (!inside(f.faceRect, f.point.x, f.point.y)) return null;
  const centreY = Math.round((f.faceRect.top + f.faceRect.bottom) / 2);
  return `${f.face} did not register: the tap was on the button but the browser `
    + `sent it to ${f.dispatchedTo ?? 'nothing'}, while ${f.elementAtPoint ?? 'nothing'} `
    + `answers at that point (pointer-events ${f.pointerEventsAtPoint ?? 'unknown'}). `
    + `Button centre y ${centreY}, touch y ${Math.round(f.point.y)}. `
    + viewportSuffix(f.viewport);
}

export function deadPressReport(f: DeadPressFacts): string | null {
  if (f.outcome !== null) return null;
  if (unclaimedVerdict(f) !== 'dead') return null;
  if (f.movedPx > TAP_MOVE_THRESHOLD_PX) return null;
  const tail = ` Held ${Math.round(f.heldMs)}ms. ${viewportSuffix(f.viewport)}`;
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

export function canceledPressReport(f: {
  face: string;
  movedPx: number;
  alone: boolean;
  heldMs: number;
  viewport: ProbeViewport;
}): string | null {
  if (unclaimedVerdict(f) !== 'dead') return null;
  if (f.movedPx > TAP_MOVE_THRESHOLD_PX) return null;
  return `${f.face} did not register: the system cancelled the touch after `
    + `${Math.round(f.movedPx)}px, held ${Math.round(f.heldMs)}ms. `
    + viewportSuffix(f.viewport);
}

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

const CLICK_GRACE_MS = 600;

const TOUCH_BEHIND_CLICK_MS = 1500;

type PressVerdict =
  | PressOutcome
  | ReturnType<typeof unclaimedVerdict>
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

const STRAY_TOUCH_THROTTLE_MS = 250;

export const LIFT_DEADLINE_MS = 4000;

function roundRect(rect: ProbeRect | null): ProbeRect | null {
  if (!rect) return null;
  return {
    left: Math.round(rect.left),
    right: Math.round(rect.right),
    top: Math.round(rect.top),
    bottom: Math.round(rect.bottom),
  };
}

interface QuietWindow {
  ms: number;
  checks: number;
  covered: number;
  nudges: number;
  closes: number;
}

let lastInputAt: number | null = null;
let checksSinceInput = 0;
let coveredSinceInput = 0;
let nudgesSinceInput = 0;
let closesAtInput = 0;
let quiet: QuietWindow | null = null;

let lastKeystrokeAt: number | null = null;

let lastComposerInputAt: number | null = null;

let nudgesSinceKeystroke = 0;

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

interface PressContext {
  morph: MorphState;
  quiet: QuietWindow | null;
  nudgesSinceKeystroke: number;
  viewport: ProbeViewport;
}

function pressContext(): PressContext {
  return {
    morph: readMorphState(),
    quiet,
    nudgesSinceKeystroke,
    viewport: readViewport(),
  };
}

function recordPress({ at, ...facts }: {
  face: string;
  verdict: PressVerdict;
  movedPx: number;
  connectedAtLift?: boolean;
  rowMutations?: number;
  dispatchedTo?: string | null;
  elementAtPoint?: string | null;
  pointerEventsAtPoint?: string | null;
  toasted?: boolean;
  rowRect?: ProbeRect | null;
  faceRect?: ProbeRect | null;
  under?: UnderFinger;
  underFace?: string | null;
  faceCount?: number;
  watchableCount?: number;
  point?: { x: number; y: number };
  missedBy?: { face: string; px: number; dx: number; dy: number; rect: ProbeRect } | null;
  screenOff?: { x: number; y: number };
  cover?: string;
  scheduled?: boolean;
  nudgeTrigger?: NudgeTrigger;
  sinceKeyboardMs?: number;
  closePath?: ClosePath | null;
  sinceInputMs?: number | null;
  nudged?: boolean;
  fingers?: number;
  fingersAtLift?: number;
  heldMs?: number;
  lateMs?: number;
  at?: PressContext;
}): void {
  postClientLog('composer-press', `${facts.face}: ${facts.verdict}`, {
    ...facts,
    movedPx: Math.round(facts.movedPx),
    ...(at ?? pressContext()),
  });
}

let lastStrayTouchAt = Number.NEGATIVE_INFINITY;

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

function faceBoxes(): { name: string; rect: ProbeRect }[] {
  return watchableFaces().map((f) => ({
    name: nameOf(f),
    rect: roundRect(f.getBoundingClientRect()) as ProbeRect,
  }));
}

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
  const nearest = rowRect ? nearestFaceMiss(faceBoxes(), landing.point) : null;
  recordPress({
    face: describe(target) ?? 'nothing',
    verdict: 'keyboard-touch',
    movedPx: 0,
    ...landing,
    missedBy: missedByOf(nearest),
  });
}

let lastCoveredTouchAt = Number.NEGATIVE_INFINITY;

function noteCoveredTouch(t: ProbeTouch, rowRect: ProbeRect | null, cover: string): void {
  const now = Date.now();
  if (now - lastCoveredTouchAt < STRAY_TOUCH_THROTTLE_MS) return;
  lastCoveredTouchAt = now;
  recordPress({
    face: 'the row',
    verdict: 'covered',
    movedPx: 0,
    cover,
    ...touchLanding(t, rowRect),
  });
}

let lastStrayClickAt = Number.NEGATIVE_INFINITY;

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

function morphElement(): HTMLButtonElement | null {
  return document.querySelector<HTMLButtonElement>(`${ROW_SELECTOR} .send-cancel-morph`);
}

function readMorphState(el = morphElement()): MorphState {
  return morphStateOf({
    present: !!el,
    disabled: !!el?.disabled,
    label: el?.getAttribute('aria-label') ?? null,
  });
}

function commitFace(): HTMLButtonElement | null {
  const morph = morphElement();
  if (morph) return readMorphState(morph) === 'send' ? morph : null;
  return document.querySelector<HTMLButtonElement>(
    `${ROW_SELECTOR} [aria-label="Submit answer"]`,
  );
}

function liveCommitFace(): HTMLButtonElement | null {
  const face = commitFace();
  if (!face || face.disabled || !face.isConnected) return null;
  return face;
}

const ROW_SELECTOR = '.prompt-actions-row';
const FACE_SELECTOR = '.action-btn';

const COMPOSER_SELECTOR = '.prompt-box';

export type FaceExclusion = 'watchable' | 'disabled';

export function faceExclusion(face: { disabled: boolean }): FaceExclusion {
  return face.disabled ? 'disabled' : 'watchable';
}

export type UnderFinger = 'disabled-face' | 'live-face' | 'other-button' | 'nothing';

export function underFingerReason(f: {
  actionFace: FaceExclusion | null;
  otherButton: boolean;
}): UnderFinger {
  if (f.actionFace === 'disabled') return 'disabled-face';
  if (f.actionFace === 'watchable') return 'live-face';
  if (f.otherButton) return 'other-button';
  return 'nothing';
}

export function missEarnsRelayout(under: UnderFinger): boolean {
  return under === 'nothing' || under === 'live-face';
}

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

export function distanceOutside(rect: ProbeRect, p: { x: number; y: number }): number {
  const { dx, dy } = missVector(rect, p);
  return Math.round(Math.hypot(dx, dy));
}

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

export function screenOffset(t: {
  screenX: number; screenY: number; clientX: number; clientY: number;
}): { x: number; y: number } {
  return { x: Math.round(t.screenX - t.clientX), y: Math.round(t.screenY - t.clientY) };
}

export type MorphState = 'absent' | 'disabled' | 'cancel' | 'send';

export function morphStateOf(m: {
  present: boolean;
  disabled: boolean;
  label: string | null;
}): MorphState {
  if (!m.present) return 'absent';
  if (m.disabled) return 'disabled';
  return m.label === 'Cancel' ? 'cancel' : 'send';
}

export function faceName(f: { ariaLabel: string | null; text: string }): string {
  const name = f.ariaLabel?.trim() || f.text.trim();
  return name.length > 0 ? name : 'A composer button';
}

function nameOf(btn: HTMLButtonElement): string {
  return faceName({ ariaLabel: btn.getAttribute('aria-label'), text: btn.textContent ?? '' });
}

function allFaces(): HTMLButtonElement[] {
  return Array.from(
    document.querySelectorAll<HTMLButtonElement>(`${ROW_SELECTOR} ${FACE_SELECTOR}`),
  );
}

function exclusionOf(btn: HTMLButtonElement): FaceExclusion {
  return faceExclusion({ disabled: btn.disabled });
}

function watchableFaces(): HTMLButtonElement[] {
  return allFaces().filter((btn) => exclusionOf(btn) === 'watchable');
}

function describe(el: Element | null): string | null {
  if (!el) return null;
  const cls = el.classList.item(0);
  return cls ? `${el.tagName.toLowerCase()}.${cls}` : el.tagName.toLowerCase();
}

function describeDispatched(el: Element | null): string | null {
  const name = describe(el);
  if (!name || !el) return name;
  return el.isConnected ? name : `${name} (detached)`;
}

function pointerEventsOf(el: Element | null): string | null {
  return el ? getComputedStyle(el).pointerEvents : null;
}

function watchableRow(): HTMLElement | null {
  const rows = document.querySelectorAll<HTMLElement>(ROW_SELECTOR);
  for (const row of rows) {
    const rect = row.getBoundingClientRect();
    if (rect.width > 0 && rect.height > 0) return row;
  }
  return null;
}

function coveredOnPurpose(): boolean {
  return coverOverShell() !== '';
}

export const SCHEDULED_CHECK_MS = 3000;

export const UNTOUCHED_QUIET_MS = 3000;

const UNTOUCHED_WINDOW_MS = 60_000;

type NudgeTrigger = 'tick' | 'keystroke';

const NUDGE_COALESCE_MS = 250;

export function nudgeIsTooSoon(lastAt: number | null, now: number): boolean {
  return lastAt !== null && now - lastAt < NUDGE_COALESCE_MS;
}

let lastNudgeAt: number | null = null;

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

const SILENCE_QUIET_MS = 3000;

export function shouldReportSilence(f: {
  hasCommitFace: boolean;
  msSinceKeyboardClose: number | null;
  msSinceInput: number | null;
}): boolean {
  if (!f.hasCommitFace) return false;
  if (f.msSinceKeyboardClose === null) return false;
  const silence = f.msSinceInput === null
    ? f.msSinceKeyboardClose
    : Math.min(f.msSinceKeyboardClose, f.msSinceInput);
  return silence >= SILENCE_QUIET_MS;
}

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
    nudged: close.relaidOut,
    sinceKeyboardMs: Math.round(now - close.at),
    sinceInputMs: msSinceInput === null ? null : Math.round(msSinceInput),
    closePath: close.path,
    faceRect: roundRect(face.getBoundingClientRect()),
  });
}

function runScheduledCheck(): void {
  if (!isMobile()) return;
  if (document.visibilityState && document.visibilityState !== 'visible') return;
  if (!watchableRow()) return;
  const faces = watchableFaces();
  if (faces.length === 0) return;
  checksSinceInput += 1;
  if (coveredOnPurpose()) { coveredSinceInput += 1; return; }
  const face = liveCommitFace();
  reportSilence(face);
  runUntouchedNudge(face, 'tick');
}

let keystrokeNudgeTimer: ReturnType<typeof setTimeout> | null = null;

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

export function _resetNudgeStateForTesting(): void {
  lastNudgeAt = null;
  lastKeystrokeAt = null;
  lastComposerInputAt = null;
  nudgesSinceKeystroke = 0;
  nudgesSinceInput = 0;
  quiet = null;
  lastInputAt = null;
  checksSinceInput = 0;
  coveredSinceInput = 0;
  if (keystrokeNudgeTimer !== null) { clearTimeout(keystrokeNudgeTimer); keystrokeNudgeTimer = null; }
}

function touchOf(e: TouchEvent, id: number): Touch | null {
  const changed = e.changedTouches;
  if (!changed) return null;
  for (let i = 0; i < changed.length; i++) {
    if (changed[i].identifier === id) return changed[i];
  }
  return null;
}

const THREAD_ACTION_SELECTOR = '[data-thread-action]';

function isThreadAction(face: Element): boolean {
  return face.closest(THREAD_ACTION_SELECTOR) !== null;
}

function warnUnlessThreadAction(report: string | null, threadAction: boolean): boolean {
  if (!report || threadAction) return false;
  showToast(report, 'warning');
  return true;
}

interface ArmedPress {
  el: HTMLButtonElement;
  face: string;
  threadAction: boolean;
  armedAt: number;
  startX: number;
  startY: number;
  movedPx: number;
  mutations: number;
  observer: MutationObserver | null;
  touchId: number;
  screenOff: { x: number; y: number };
  fingers: number;
  faceRect: ProbeRect | null;
  rowRect: ProbeRect | null;
  at: PressContext;
  liftDeadline: ReturnType<typeof setTimeout> | null;
}

interface SettlingPress {
  el: HTMLButtonElement;
  face: string;
  threadAction: boolean;
  armedAt: number;
  movedPx: number;
  connectedAtLift: boolean;
  rowMutations: number;
  screenOff: { x: number; y: number };
  fingers: number;
  fingersAtLift: number;
  heldMs: number;
  liftedAt: number;
  faceRect: ProbeRect | null;
  rowRect: ProbeRect | null;
  at: PressContext;
  outcome: PressOutcome | null;
  outcomeTimer: ReturnType<typeof setTimeout> | null;
  graceTimer: ReturnType<typeof setTimeout> | null;
}

interface ArmedMiss {
  touchId: number;
  startX: number;
  startY: number;
  movedPx: number;
  fingers: number;
  fingersAtLift: number;
}

let installed = false;

export function installDeadPressProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;

  let armed: ArmedPress | null = null;
  let missedPress: ArmedMiss | null = null;
  const settling = new Set<SettlingPress>();
  let lastTouchStartAt: number | null = null;

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
        heldMs: press.heldMs,
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
      heldMs: press.heldMs,
      viewport: press.at.viewport,
    });
    recordPress({
      face: press.face,
      verdict: outcome ?? unclaimedVerdict({ alone, heldMs: press.heldMs }),
      movedPx: press.movedPx,
      connectedAtLift: press.connectedAtLift,
      rowMutations: press.rowMutations,
      ...fingers,
      heldMs: press.heldMs,
      lateMs: Math.max(0, Math.round(Date.now() - press.liftedAt - CLICK_GRACE_MS)),
      toasted: warnUnlessThreadAction(report, press.threadAction),
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
      at: press.at,
    });
  };

  const ruleMissedPress = (miss: ArmedMiss) => {
    if (miss.movedPx > TAP_MOVE_THRESHOLD_PX) return;
    if (!pressWasAlone(miss)) return;
    relayoutShell();
  };

  const disarm = (press: ArmedPress) => {
    press.observer?.disconnect();
    press.observer = null;
    if (press.liftDeadline !== null) { clearTimeout(press.liftDeadline); press.liftDeadline = null; }
  };

  const ruleArmedWithNoLift = (press: ArmedPress, toast: boolean) => {
    disarm(press);
    const report = noLiftReport({
      face: press.face,
      movedPx: press.movedPx,
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
      toasted: warnUnlessThreadAction(toast ? report : null, press.threadAction),
      rowRect: press.rowRect,
      faceRect: press.faceRect,
      screenOff: press.screenOff,
      at: press.at,
    });
  };

  installPressCoverWatch();

  document.addEventListener('touchstart', (e) => {
    const cover = takeCoverForThisPress();
    const live = armed ?? missedPress;
    if (live && e.touches.length > 1) {
      live.fingers = Math.max(live.fingers, e.touches.length);
      return;
    }
    const previous = armed;
    armed = null;
    if (previous) ruleArmedWithNoLift(previous, true);
    missedPress = null;
    if (!isMobile()) return;
    lastTouchStartAt = Date.now();
    noteInput(lastTouchStartAt);
    const touch = e.changedTouches?.[0];
    if (!touch) return;
    const target = e.target as Element | null;
    if (target) hearTouchOnTarget(target);
    const row = watchableRow();
    if (!row) { noteStrayTouch(touch, target, null); return; }
    const rowRect = row.getBoundingClientRect();
    const onRow = !!target && !!target.closest(ROW_SELECTOR);
    const inRow = inside(rowRect, touch.clientX, touch.clientY);
    if (inRow || !!target?.closest(COMPOSER_SELECTOR)) {
      lastComposerInputAt = lastTouchStartAt;
    }
    const every = allFaces();
    const faces = every.filter((btn) => exclusionOf(btn) === 'watchable');
    const pressed = faces.find((f) => !!target && (target === f || f.contains(target)));
    if (!pressed) {
      const covered = cover !== '';
      if (!onRow && !inRow) {
        if (!covered) noteStrayTouch(touch, target, roundRect(rowRect));
        return;
      }
      if (covered) { noteCoveredTouch(touch, roundRect(rowRect), cover); return; }
      const boxes = faces.map((f) => ({ el: f, name: nameOf(f), rect: roundRect(f.getBoundingClientRect()) as ProbeRect }));
      const aimedAt = boxes.find((b) => inside(b.rect, touch.clientX, touch.clientY)) ?? null;
      const at = document.elementFromPoint(touch.clientX, touch.clientY);
      const elementAtPoint = describe(at);
      const pointerEventsAtPoint = pointerEventsOf(at);
      const dispatchedTo = describeDispatched(target);
      const report = aimedAt ? landingReport({
        face: aimedAt.name,
        point: { x: touch.clientX, y: touch.clientY },
        faceRect: aimedAt.rect,
        targetIsFace: false,
        dispatchedTo,
        elementAtPoint,
        pointerEventsAtPoint,
        viewport: readViewport(),
      }) : null;
      const held = every.find((f) => inside(f.getBoundingClientRect(), touch.clientX, touch.clientY));
      const under = underFingerReason({
        actionFace: held ? exclusionOf(held) : null,
        otherButton: !!at && at.closest('button') !== null,
      });
      const point = { x: Math.round(touch.clientX), y: Math.round(touch.clientY) };
      const nearest = nearestFaceMiss(boxes, point);
      const pressAt = pressContext();
      recordPress({
        face: aimedAt ? aimedAt.name : 'the row',
        verdict: 'missed',
        movedPx: 0,
        dispatchedTo,
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
        toasted: warnUnlessThreadAction(report, !!aimedAt && isThreadAction(aimedAt.el)),
        at: pressAt,
      });
      missedPress = !missEarnsRelayout(under) ? null : {
        touchId: touch.identifier,
        startX: touch.screenX,
        startY: touch.screenY,
        movedPx: 0,
        fingers: e.touches.length,
        fingersAtLift: 0,
      };
      return;
    }
    const press: ArmedPress = {
      el: pressed,
      face: nameOf(pressed),
      threadAction: isThreadAction(pressed),
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
      ruleArmedWithNoLift(press, false);
    }, LIFT_DEADLINE_MS);
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

  let targetListeners: AbortController | null = null;
  const hearTouchOnTarget = (target: Element) => {
    targetListeners?.abort();
    targetListeners = new AbortController();
    const { signal } = targetListeners;
    const handle = (handler: (e: TouchEvent) => void) => (e: Event) => handler(e as TouchEvent);
    target.addEventListener('touchmove', handle(onTouchMove), { passive: true, signal });
    target.addEventListener('touchend', handle(onTouchEnd), { passive: true, signal });
    target.addEventListener('touchcancel', handle(onTouchCancel), { passive: true, signal });
  };

  const onTouchMove = (e: TouchEvent) => {
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
  };
  document.addEventListener('touchmove', onTouchMove, { capture: true, passive: true });

  const onTouchEnd = (e: TouchEvent) => {
    const miss = missedPress;
    if (miss && touchOf(e, miss.touchId)) {
      missedPress = null;
      miss.fingersAtLift = e.touches.length;
      ruleMissedPress(miss);
    }
    const press = armed;
    if (!press || !touchOf(e, press.touchId)) return;
    armed = null;
    disarm(press);
    const lifted: SettlingPress = {
      el: press.el,
      face: press.face,
      threadAction: press.threadAction,
      armedAt: press.armedAt,
      movedPx: press.movedPx,
      connectedAtLift: press.el.isConnected,
      rowMutations: press.mutations,
      screenOff: press.screenOff,
      fingers: press.fingers,
      fingersAtLift: e.touches.length,
      heldMs: Date.now() - press.armedAt,
      liftedAt: Date.now(),
      faceRect: press.faceRect,
      rowRect: press.rowRect,
      at: press.at,
      outcome: null,
      outcomeTimer: null,
      graceTimer: null,
    };
    settling.add(lifted);
    lifted.outcomeTimer = setTimeout(() => {
      lifted.outcomeTimer = null;
      lifted.outcome = takePressOutcome(Date.now() - lifted.armedAt);
    }, 0);
    lifted.graceTimer = setTimeout(() => {
      lifted.graceTimer = null;
      rule(lifted, false);
    }, CLICK_GRACE_MS);
  };
  document.addEventListener('touchend', onTouchEnd, { capture: true, passive: true });

  const onTouchCancel = (e: TouchEvent) => {
    if (missedPress && touchOf(e, missedPress.touchId)) missedPress = null;
    const press = armed;
    if (!press || !touchOf(e, press.touchId)) return;
    armed = null;
    disarm(press);
    const heldMs = Date.now() - press.armedAt;
    const fingersAtLift = e.touches.length;
    setTimeout(() => {
      const outcome = takePressOutcome(Date.now() - press.armedAt);
      const report = outcome !== null ? null : canceledPressReport({
        face: press.face,
        movedPx: press.movedPx,
        alone: pressWasAlone({ fingers: press.fingers, fingersAtLift }),
        heldMs,
        viewport: press.at.viewport,
      });
      recordPress({
        face: press.face,
        verdict: outcome ?? 'canceled',
        movedPx: press.movedPx,
        fingers: press.fingers,
        fingersAtLift,
        heldMs,
        toasted: warnUnlessThreadAction(report, press.threadAction),
        rowRect: press.rowRect,
        faceRect: press.faceRect,
        screenOff: press.screenOff,
        at: press.at,
      });
    }, 0);
  };
  document.addEventListener('touchcancel', onTouchCancel, { capture: true, passive: true });

  document.addEventListener('click', (e) => {
    const target = e.target as Element | null;
    if (!target) return;
    if (target.closest(COMPOSER_SELECTOR)) lastComposerInputAt = Date.now();
    const touchBehind = lastTouchStartAt !== null
      && Date.now() - lastTouchStartAt < TOUCH_BEHIND_CLICK_MS;
    if (!touchBehind) noteInput(Date.now());
    for (const press of Array.from(settling).reverse()) {
      const onPressedFace = press.el.contains(target);
      const onReplacement = !press.el.isConnected && !!target.closest(ROW_SELECTOR);
      if (!onPressedFace && !onReplacement) continue;
      rule(press, true);
      return;
    }
    if (!isMobile() || !isTouchDevice()) return;
    if (touchBehind) return;
    const face = watchableFaces().find((f) => target === f || f.contains(target));
    if (!face) {
      noteStrayClick(target, clickLanding(e));
      return;
    }
    recordPress({
      face: nameOf(face),
      verdict: 'click-no-touch',
      movedPx: 0,
      rowRect: roundRect(watchableRow()?.getBoundingClientRect() ?? null),
      faceRect: roundRect(face.getBoundingClientRect()),
    });
  }, { capture: true, passive: true });

  document.addEventListener('input', (e) => {
    const el = e.target as HTMLElement | null;
    if (el?.dataset?.role !== 'prompt-input') return;
    lastKeystrokeAt = Date.now();
    nudgesSinceKeystroke = 0;
    armKeystrokeNudge();
  }, { capture: true, passive: true });

  setInterval(runScheduledCheck, SCHEDULED_CHECK_MS);
}
