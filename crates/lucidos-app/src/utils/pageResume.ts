import { isIOS, isWebKit } from './platform';
import { OPEN_REPAINT_BURST_DELAYS_MS } from './repaintBurst';

export const IRREVERSIBLE_TAP_SELECTOR = '.question-option, .permission-actions button';

export function isIrreversibleTapTarget(target: EventTarget | null): boolean {
  const el = target as { closest?: (s: string) => unknown } | null;
  return !!(el && typeof el.closest === 'function' && el.closest(IRREVERSIBLE_TAP_SELECTOR));
}

export type ResumeEvent =
  | { kind: 'resume' }
  | { kind: 'settled' }
  | { kind: 'click'; irreversible: boolean };

export interface ResumeEffect {
  swallow: boolean;
  repaint: boolean;
}

export function reduceResumeGuard(
  armed: boolean,
  ev: ResumeEvent,
): { armed: boolean; effect: ResumeEffect } {
  switch (ev.kind) {
    case 'resume':
      return { armed: true, effect: { swallow: false, repaint: true } };
    case 'settled':
      return { armed: false, effect: { swallow: false, repaint: false } };
    case 'click':
      if (!armed) return { armed: false, effect: { swallow: false, repaint: false } };
      return { armed: false, effect: { swallow: ev.irreversible, repaint: true } };
  }
}


const repaintSubscribers = new Set<() => void>();
let armed = false;
let installed = false;
let settleScheduled = false;

function fireRepaint(): void {
  for (const cb of repaintSubscribers) cb();
}

function step(ev: ResumeEvent, domEvent?: Event): void {
  const { armed: next, effect } = reduceResumeGuard(armed, ev);
  armed = next;
  if (effect.repaint) fireRepaint();
  if (effect.swallow && domEvent) {
    domEvent.stopPropagation();
    domEvent.preventDefault();
  }
}

function scheduleSettle(): void {
  if (settleScheduled || typeof requestAnimationFrame !== 'function') return;
  settleScheduled = true;
  requestAnimationFrame(() =>
    requestAnimationFrame(() => {
      settleScheduled = false;
      step({ kind: 'settled' });
    }),
  );
}

function onResume(): void {
  step({ kind: 'resume' });
  scheduleSettle();
}

let focusInFrame = false;
let blurCheck: ReturnType<typeof setTimeout> | undefined;

function onWindowBlur(): void {
  clearTimeout(blurCheck);
  blurCheck = setTimeout(() => { focusInFrame = document.activeElement?.tagName === 'IFRAME'; }, 0);
}
function onWindowFocus(): void {
  const fromFrame = focusInFrame;
  focusInFrame = false;
  if (fromFrame) fireRepaint();
  else onResume();
}
function onPageHide(): void {
  clearTimeout(blurCheck);
  focusInFrame = false;
}
function onVisibilityChange(): void {
  if (document.visibilityState === 'visible') onResume();
  else onPageHide();
}
function onClickCapture(e: MouseEvent): void {
  step({ kind: 'click', irreversible: isIrreversibleTapTarget(e.target) }, e);
}

function install(): void {
  if (installed || !isWebKit()) return;
  installed = true;
  document.addEventListener('visibilitychange', onVisibilityChange);
  window.addEventListener('pageshow', onResume);
  window.addEventListener('pagehide', onPageHide);
  window.addEventListener('focus', onWindowFocus);
  window.addEventListener('blur', onWindowBlur);
  if (isIOS()) document.addEventListener('click', onClickCapture, true);
}

export function onPageResume(cb: () => void): () => void {
  install();
  repaintSubscribers.add(cb);
  return () => {
    repaintSubscribers.delete(cb);
  };
}

export function repaintLandedContent(): void {
  if (!isWebKit()) return;
  for (const delay of OPEN_REPAINT_BURST_DELAYS_MS) {
    if (delay === 0) fireRepaint();
    else setTimeout(fireRepaint, delay);
  }
}
