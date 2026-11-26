import { USER_SCROLL_WINDOW_MS, isScrollKeyPress, nowMs } from '../../utils/scrollActivity';

type Timer = ReturnType<typeof setTimeout>;

type Fingers = Map<number, EventTarget | null>;

type ReaderInput =
  | { kind: 'idle' }
  | { kind: 'touching'; cap: Timer; fingers: Fingers }
  | { kind: 'coasting'; still: Timer }
  | { kind: 'scrollbar'; held: boolean; gutter: boolean };

interface ReaderInputEntry {
  input: ReaderInput;
  scrollOwedSince: number;
  movedAt: number;
}

const _entries = new Map<HTMLElement, ReaderInputEntry>();

function entryOf(el: HTMLElement): ReaderInputEntry {
  let entry = _entries.get(el);
  if (!entry) {
    entry = { input: { kind: 'idle' }, scrollOwedSince: -Infinity, movedAt: -Infinity };
    _entries.set(el, entry);
  }
  return entry;
}

const READER_MOVEMENT_TAIL_MS = USER_SCROLL_WINDOW_MS;

export const TOUCH_COAST_IDLE_MS = 150;

export const TOUCH_COAST_MAX_MS = 4000;

const _scrollHoldReleaseListeners = new Set<(el: HTMLElement) => void>();

function clearTouchTimers(input: ReaderInput) {
  if (input.kind === 'touching') clearTimeout(input.cap);
  if (input.kind === 'coasting') clearTimeout(input.still);
}

function holds(input: ReaderInput): boolean {
  return input.kind === 'touching' || input.kind === 'coasting'
    || (input.kind === 'scrollbar' && input.held);
}

function goIdle(el: HTMLElement, entry: ReaderInputEntry) {
  const wasHeld = holds(entry.input);
  clearTouchTimers(entry.input);
  entry.input = { kind: 'idle' };
  if (wasHeld) for (const listener of _scrollHoldReleaseListeners) listener(el);
}

function releaseScrollbar(el: HTMLElement) {
  const entry = _entries.get(el);
  if (entry?.input.kind === 'scrollbar') goIdle(el, entry);
}

function releaseTouch(el: HTMLElement) {
  const entry = _entries.get(el);
  if (entry?.input.kind === 'touching' || entry?.input.kind === 'coasting') goIdle(el, entry);
}

function moved(el: HTMLElement) {
  entryOf(el).movedAt = nowMs();
}

export const READER_SCROLL_EVENT_WAIT_MS = 100;

function movedAwaitingScroll(el: HTMLElement) {
  const entry = entryOf(el);
  entry.movedAt = nowMs();
  entry.scrollOwedSince = entry.movedAt;
}

export function readerScrollOwed(el: HTMLElement): number {
  const entry = _entries.get(el);
  if (!entry) return 0;
  return Math.max(0, READER_SCROLL_EVENT_WAIT_MS - (nowMs() - entry.scrollOwedSince));
}

function fingersStillDown(
  el: HTMLElement, fingers: Fingers, touches: TouchList | undefined,
): boolean {
  if (!touches) return false;
  if (fingers.size === 0) return touches.length > 0;
  const down: Fingers = new Map(Array.from(touches, (touch) => [touch.identifier, touch.target]));
  for (const [finger, target] of fingers) {
    if (!down.has(finger) || down.get(finger) !== target) fingers.delete(finger);
  }
  nameFingersOn(el, fingers, touches);
  return fingers.size > 0;
}

function nameFingersOn(el: HTMLElement, fingers: Fingers, touches: TouchList | undefined) {
  for (const touch of Array.from(touches ?? [])) {
    if (el.contains?.(touch.target as Node | null)) fingers.set(touch.identifier, touch.target);
  }
}

export function isScrollHeld(el: HTMLElement): boolean {
  const entry = _entries.get(el);
  return entry !== undefined && holds(entry.input);
}

function scrollbarHeld(el: HTMLElement): boolean {
  const input = _entries.get(el)?.input;
  return input?.kind === 'scrollbar' && input.held;
}

export function onScrollHoldReleased(listener: (el: HTMLElement) => void): () => void {
  _scrollHoldReleaseListeners.add(listener);
  return () => { _scrollHoldReleaseListeners.delete(listener); };
}

export function scrollHoldReleased(el: HTMLElement): Promise<void> {
  if (!isScrollHeld(el)) return Promise.resolve();
  return new Promise((resolve) => {
    const unsubscribe = onScrollHoldReleased((released) => {
      if (released !== el) return;
      unsubscribe();
      requestAnimationFrame(() => resolve());
    });
  });
}

export function retireReaderMovement() {
  for (const entry of _entries.values()) entry.movedAt = -Infinity;
}

export function readerGestureActive(el: HTMLElement): boolean {
  if (scrollbarHeld(el)) return true;
  const entry = _entries.get(el);
  return entry !== undefined && nowMs() - entry.movedAt < READER_MOVEMENT_TAIL_MS;
}

export function readerGestureSince(el: HTMLElement, since: number): boolean {
  if (scrollbarHeld(el)) return true;
  const entry = _entries.get(el);
  return entry !== undefined && entry.movedAt > since;
}

export function attachReaderInput(el: HTMLElement): () => void {
  const root = typeof window !== 'undefined' && typeof window.addEventListener === 'function'
    ? window
    : null;
  if (typeof el.addEventListener !== 'function' || !root) return () => {};
  const onDown = (e: PointerEvent) => {
    if (e.target !== el) return;
    const held = e.button === 0;
    const gutter = e.offsetX > el.clientWidth;
    if (!held && !gutter) return;
    const entry = entryOf(el);
    if (gutter) entry.movedAt = nowMs();
    const current = entry.input;
    if (current.kind === 'touching') return;
    if (current.kind === 'coasting' && !held) return;
    clearTouchTimers(current);
    entry.input = current.kind === 'scrollbar'
      ? { kind: 'scrollbar', held: held || current.held, gutter: gutter || current.gutter }
      : { kind: 'scrollbar', held, gutter };
  };
  const onDrag = (e: PointerEvent) => {
    if (e.buttons === 0) { releaseScrollbar(el); return; }
    const entry = _entries.get(el);
    if (entry?.input.kind === 'scrollbar' && entry.input.gutter) entry.movedAt = nowMs();
  };
  const onWheel = () => movedAwaitingScroll(el);
  const capTimer = () => setTimeout(() => releaseTouch(el), TOUCH_COAST_MAX_MS);
  const stillTimer = () => setTimeout(() => releaseTouch(el), TOUCH_COAST_IDLE_MS);
  const onTouchMove = (e: TouchEvent) => {
    const entry = entryOf(el);
    entry.movedAt = nowMs();
    const fingers: Fingers = entry.input.kind === 'touching' ? entry.input.fingers : new Map();
    for (const touch of Array.from(e.changedTouches ?? [])) fingers.set(touch.identifier, touch.target);
    nameFingersOn(el, fingers, e.touches);
    clearTouchTimers(entry.input);
    entry.input = { kind: 'touching', cap: capTimer(), fingers };
  };
  const onTouchStart = (e: TouchEvent) => {
    const input = _entries.get(el)?.input;
    if (input?.kind !== 'touching') return;
    for (const touch of Array.from(e.changedTouches ?? [])) input.fingers.set(touch.identifier, touch.target);
  };
  const onScroll = () => {
    const entry = _entries.get(el);
    if (entry) entry.scrollOwedSince = -Infinity;
    const input = entry?.input;
    if (input?.kind !== 'coasting') return;
    clearTimeout(input.still);
    input.still = stillTimer();
  };
  const onUp = () => releaseScrollbar(el);
  const onTouchUp = (e: TouchEvent) => {
    const entry = _entries.get(el);
    if (entry?.input.kind !== 'touching') return;
    if (fingersStillDown(el, entry.input.fingers, e.touches)) return;
    clearTouchTimers(entry.input);
    entry.input = { kind: 'coasting', still: stillTimer() };
  };
  let blurTimer: Timer | undefined;
  const onBlur = () => {
    clearTimeout(blurTimer);
    blurTimer = setTimeout(() => {
      if (typeof document === 'undefined' || !document.hasFocus()) releaseScrollbar(el);
    }, 0);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.target === el && isScrollKeyPress(e)) movedAwaitingScroll(el);
  };

  el.addEventListener('pointerdown', onDown as EventListener, { passive: true });
  el.addEventListener('pointermove', onDrag as EventListener, { passive: true });
  el.addEventListener('touchstart', onTouchStart as EventListener, { passive: true });
  el.addEventListener('touchmove', onTouchMove as EventListener, { passive: true });
  el.addEventListener('wheel', onWheel, { passive: true });
  el.addEventListener('scroll', onScroll, { passive: true });
  el.addEventListener('keydown', onKey);
  root.addEventListener('pointerup', onUp, { passive: true });
  root.addEventListener('pointercancel', onUp, { passive: true });
  root.addEventListener('touchend', onUp, { passive: true });
  root.addEventListener('touchcancel', onUp, { passive: true });
  root.addEventListener('touchend', onTouchUp as EventListener, { passive: true });
  root.addEventListener('touchcancel', onTouchUp as EventListener, { passive: true });
  root.addEventListener('blur', onBlur);

  return () => {
    el.removeEventListener('pointerdown', onDown as EventListener);
    el.removeEventListener('pointermove', onDrag as EventListener);
    el.removeEventListener('touchstart', onTouchStart as EventListener);
    el.removeEventListener('touchmove', onTouchMove as EventListener);
    el.removeEventListener('wheel', onWheel);
    el.removeEventListener('scroll', onScroll);
    el.removeEventListener('keydown', onKey);
    root.removeEventListener('pointerup', onUp);
    root.removeEventListener('pointercancel', onUp);
    root.removeEventListener('touchend', onUp);
    root.removeEventListener('touchcancel', onUp);
    root.removeEventListener('touchend', onTouchUp as EventListener);
    root.removeEventListener('touchcancel', onTouchUp as EventListener);
    root.removeEventListener('blur', onBlur);
    clearTimeout(blurTimer);
    const entry = _entries.get(el);
    if (entry) goIdle(el, entry);
    _entries.delete(el);
  };
}

export function readerGestureForTest(el: HTMLElement | null, isMoved = true): void {
  if (!el || !isMoved) {
    for (const entry of _entries.values()) clearTouchTimers(entry.input);
    _entries.clear();
    return;
  }
  moved(el);
}
