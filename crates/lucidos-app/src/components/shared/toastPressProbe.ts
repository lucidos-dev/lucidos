import { postClientLog } from '../../utils/clientLog';

/** Logs every press on a toast control to `engine.log`, and what became of it.
 *
 *  A DIAGNOSTIC, registered in `docs/temporary-measures.md` § 1. On the iOS PWA
 *  a toast's Open sometimes does nothing on the first tap and works on the
 *  second. The failed tap left no trace, so nothing said whether iOS delivered
 *  the touch, whether a click followed, or what cancelled it.
 *
 *  It takes no gesture: every listener is passive, and it calls neither
 *  `preventDefault` nor `stopPropagation`. The document capture listeners go
 *  in at boot, ahead of any overlay's, so a later swallow cannot hide a press.
 *
 *  It logs the control's label and never the toast's message. */

/** What a toast press ended as.
 *
 *  - `clicked`: the click arrived and nothing cancelled it.
 *  - `click-cancelled`: the click arrived and a listener cancelled it.
 *  - `no-click`: the finger lifted and no click followed.
 *  - `touch-cancelled`: the system took the gesture.
 *  - `no-lift`: the lift never arrived. */
export type ToastPressVerdict =
  | 'clicked'
  | 'click-cancelled'
  | 'no-click'
  | 'touch-cancelled'
  | 'no-lift';

export function toastPressVerdict(f: {
  lifted: boolean;
  touchCancelled: boolean;
  clicked: boolean;
  clickPrevented: boolean;
}): ToastPressVerdict {
  if (f.clicked) return f.clickPrevented ? 'click-cancelled' : 'clicked';
  if (f.touchCancelled) return 'touch-cancelled';
  return f.lifted ? 'no-click' : 'no-lift';
}

/** Longer than WebKit's synthetic-click delay after a lift. */
const CLICK_GRACE_MS = 600;
/** Far beyond any tap, so only a lost lift reaches it. */
const LIFT_DEADLINE_MS = 4000;

const TOAST_CONTROL_SELECTOR = '.toast button, .toast .toast-clickable';

interface Press {
  control: Element;
  label: string;
  touchId: number;
  startAt: number;
  startX: number;
  startY: number;
  movedPx: number;
  fingers: number;
  overlayOpen: boolean;
  lifted: boolean;
  liftMs: number | null;
  touchCancelled: boolean;
  clicked: boolean;
  clickPrevented: boolean;
  /** Page-wide DOM mutations since touchdown, and the count at the lift. */
  mutations: number;
  mutationsAtLift: number | null;
  observer: MutationObserver | null;
  timer: ReturnType<typeof setTimeout>;
}

let press: Press | null = null;

function toastControlOf(target: EventTarget | null): Element | null {
  const node = target as Node | null;
  const el = node?.nodeType === 1 ? (node as Element) : node?.parentElement;
  return el?.closest?.(TOAST_CONTROL_SELECTOR) ?? null;
}

/** A button's label, never the body's text: the body IS the toast's message. */
function labelOf(el: Element): string {
  if (el.tagName !== 'BUTTON') return 'toast body';
  return el.getAttribute('aria-label')?.trim() || el.textContent?.trim().slice(0, 40) || 'button';
}

function overlayOpen(): boolean {
  return document.documentElement.hasAttribute('data-overlay-open');
}

function settle(p: Press): void {
  if (press !== p) return;
  press = null;
  clearTimeout(p.timer);
  p.observer?.disconnect();
  const verdict = toastPressVerdict(p);
  postClientLog('toast-press', `${p.label}: ${verdict}`, {
    face: p.label,
    verdict,
    movedPx: Math.round(p.movedPx),
    fingers: p.fingers,
    liftMs: p.liftMs,
    overlayOpen: p.overlayOpen,
    overlayOpenAtSettle: overlayOpen(),
    mutationsAtLift: p.mutationsAtLift,
    mutations: p.mutations,
    connected: p.control.isConnected,
  });
}

function onTouchStart(e: TouchEvent): void {
  const control = toastControlOf(e.target);
  if (!control) return;
  if (press) settle(press);
  const t = e.changedTouches[0];
  if (!t) return;
  const p: Press = {
    control,
    label: labelOf(control),
    touchId: t.identifier,
    startAt: Date.now(),
    startX: t.screenX,
    startY: t.screenY,
    movedPx: 0,
    fingers: e.touches.length,
    overlayOpen: overlayOpen(),
    lifted: false,
    liftMs: null,
    touchCancelled: false,
    clicked: false,
    clickPrevented: false,
    mutations: 0,
    mutationsAtLift: null,
    observer: null,
    timer: setTimeout(() => settle(p), LIFT_DEADLINE_MS),
  };
  // Counts what changed on the page while the finger was down. WebKit drops a
  // tap's click when content changes under it, so this names that case.
  if (typeof MutationObserver !== 'undefined') {
    p.observer = new MutationObserver((records) => { p.mutations += records.length; });
    p.observer.observe(document.body, { childList: true, subtree: true, attributes: true, characterData: true });
  }
  press = p;
}

function touchOf(e: TouchEvent, p: Press): Touch | null {
  for (const t of Array.from(e.changedTouches)) if (t.identifier === p.touchId) return t;
  return null;
}

function onTouchMove(e: TouchEvent): void {
  const p = press;
  if (!p) return;
  p.fingers = Math.max(p.fingers, e.touches.length);
  const t = touchOf(e, p);
  if (!t) return;
  p.movedPx = Math.max(p.movedPx, Math.abs(t.screenX - p.startX), Math.abs(t.screenY - p.startY));
}

function onTouchEnd(e: TouchEvent): void {
  const p = press;
  if (!p || p.lifted || !touchOf(e, p)) return;
  p.lifted = true;
  p.liftMs = Date.now() - p.startAt;
  p.mutationsAtLift = p.mutations;
  clearTimeout(p.timer);
  p.timer = setTimeout(() => settle(p), CLICK_GRACE_MS);
}

function onTouchCancel(e: TouchEvent): void {
  const p = press;
  if (!p || !touchOf(e, p)) return;
  p.touchCancelled = true;
  settle(p);
}

function onClick(e: MouseEvent): void {
  const p = press;
  if (!p || toastControlOf(e.target) !== p.control) return;
  p.clicked = true;
  // Read after the whole dispatch, since a cancel can come from any listener.
  setTimeout(() => {
    p.clickPrevented = e.defaultPrevented;
    settle(p);
  }, 0);
}

let installed = false;

export function installToastPressProbe(): void {
  if (installed || typeof document === 'undefined') return;
  installed = true;
  const passiveCapture = { capture: true, passive: true };
  document.addEventListener('touchstart', onTouchStart, passiveCapture);
  document.addEventListener('touchmove', onTouchMove, passiveCapture);
  document.addEventListener('touchend', onTouchEnd, passiveCapture);
  document.addEventListener('touchcancel', onTouchCancel, passiveCapture);
  document.addEventListener('click', onClick, passiveCapture);
}
