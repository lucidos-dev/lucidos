/** Relayout the shell when the software keyboard closes.
 *
 *  Goes with the dead-press probe at the end of its quiet period: see that row
 *  in `docs/temporary-measures.md`. ADR 0262 found the dead composer's cause.
 *  iOS autocorrect keeps the tap on a button below the text, and no relayout
 *  reaches that. The keyboard close watched here was the user's recovery, not
 *  the trigger.
 *
 *  Until then it relayouts the shell on each close and stamps the close for the
 *  probe's `silent-since-keyboard` line. A leaf: it imports nothing, so the
 *  probe can read its stamp without taking anything else. */

const RESTORED_SLACK_PX = 4;

const KEYBOARD_COVER_MIN_PX = 100;

export interface ViewportSample {
  height: number;
  layoutViewport: number;
}

export type CoverWatch = number | null;

export function foldViewportReading(
  watch: CoverWatch,
  sample: ViewportSample,
): { watch: CoverWatch; closed: boolean; covered: boolean } {
  const full = sample.layoutViewport;
  if (!Number.isFinite(full) || full <= 0 || !Number.isFinite(sample.height) || sample.height <= 0) {
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

export function bounceHeight(current: number, layoutViewport: number): number {
  const keyboard = Math.round(layoutViewport - current);
  const span = keyboard > 0 ? keyboard : 1;
  return Math.max(1, Math.round(current - span));
}

export function relayoutShell(): boolean {
  const root = document.documentElement;
  const prior = root.style.getPropertyValue('--app-height');
  if (!/^-?[\d.]+px$/.test(prior.trim())) return false;
  const px = Number.parseFloat(prior);
  if (!Number.isFinite(px)) return false;
  const inner = window.innerHeight;
  const away = Number.isFinite(inner) && inner > 0 ? bounceHeight(px, inner) : Math.max(1, px - 1);
  root.style.setProperty('--app-height', `${away}px`);
  void root.offsetHeight;
  root.style.setProperty('--app-height', prior);
  void root.offsetHeight;
  if (typeof window.scrollTo === 'function') window.scrollTo(0, window.scrollY);
  return true;
}

let watch: CoverWatch = null;

let closedAt: number | null = null;
let closeRelaidOut = false;
let closes = 0;
let closePath: ClosePath | null = null;

const WAKE_ECHO_MS = 600;
let wakeEchoUntil = 0;

export type ClosePath = 'resize' | 'wake' | 'poll';

export interface KeyboardCloseState {
  at: number | null;
  relaidOut: boolean;
  closes: number;
  path: ClosePath | null;
}

export function keyboardCloseState(): KeyboardCloseState {
  return { at: closedAt, relaidOut: closeRelaidOut, closes, path: closePath };
}

function recordClose(path: ClosePath, now: number): void {
  closedAt = now;
  closePath = path;
  closes += 1;
  closeRelaidOut = relayoutShell();
}

function retireClose(): void {
  closedAt = null;
  closeRelaidOut = false;
  closePath = null;
}

function takeReading(sample: ViewportSample, path: ClosePath, now: number): void {
  const folded = foldViewportReading(watch, sample);
  if (folded.covered && now < wakeEchoUntil) return;
  watch = folded.watch;
  if (folded.covered) retireClose();
  if (folded.closed) recordClose(path, now);
}

export function noteViewportResize(sample: ViewportSample, now = Date.now()): void {
  takeReading(sample, 'resize', now);
}

export function notePolledViewport(sample: ViewportSample, now = Date.now()): void {
  takeReading(sample, 'poll', now);
}

function openWakeWindow(now: number): boolean {
  const paired = now < wakeEchoUntil;
  wakeEchoUntil = now + WAKE_ECHO_MS;
  return paired && closedAt !== null;
}

export function noteWakeViewport(sample: ViewportSample, now = Date.now()): void {
  if (openWakeWindow(now)) return;
  takeReading(sample, 'wake', now);
}

export function noteKeyboardClosed(now = Date.now()): void {
  if (openWakeWindow(now)) return;
  watch = null;
  recordClose('wake', now);
}

export function resetKeyboardCloseState(): void {
  watch = null;
  closedAt = null;
  closeRelaidOut = false;
  closes = 0;
  closePath = null;
  wakeEchoUntil = 0;
}
