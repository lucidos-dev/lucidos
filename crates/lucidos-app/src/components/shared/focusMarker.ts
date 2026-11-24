
import { watchUserAction } from '../../utils/userAction';
import { isReducedMotion, scaledDurationMs } from '../../utils/motion';

const NAV_FOCUS_STUCK_CLASS = 'nav-focus-stuck';
const NAV_FOCUS_FADING_CLASS = 'nav-focus-fading';
export const NAV_FOCUS_FADE_MS = 800;

export const NAV_FOCUS_RAMP_MS = 450;

export const NAV_FOCUS_HOLD_MS = 2000;

let _markedEl: HTMLElement | null = null;
let _refLive = false;
let _teardown: (() => void) | null = null;
let _holdOver = false;
let _dismissQueued = false;
let _cancelRefExpiry: (() => void) | null = null;
let _generation = 0;

export function hasNavFocus(): boolean {
  return navFocusElement() !== null;
}

export function navFocusElement(): HTMLElement | null {
  return _refLive ? _markedEl : null;
}

function retireRefNextFrame(gen: number, dropEl: boolean): void {
  _cancelRefExpiry?.();
  _cancelRefExpiry = null;
  const expire = () => {
    if (_generation !== gen) return;
    _refLive = false;
    if (dropEl) _markedEl = null;
  };
  if (typeof requestAnimationFrame === 'function') {
    const raf = requestAnimationFrame(expire);
    _cancelRefExpiry = () => cancelAnimationFrame(raf);
  } else {
    expire();
  }
}

export function applyNavFocus(el: HTMLElement, opts?: { settleGuard?: () => boolean }): void {
  clearNavFocus();
  el.classList.remove(NAV_FOCUS_FADING_CLASS);
  el.classList.add(NAV_FOCUS_STUCK_CLASS);
  _markedEl = el;
  _refLive = true;
  _generation++;
  _holdOver = false;
  _dismissQueued = false;

  const gen = _generation;
  const settleGuard = opts?.settleGuard;
  const removeListeners = watchUserAction(() => {
    if (settleGuard?.()) return;
    if (!_holdOver) {
      if (_dismissQueued) return;
      _dismissQueued = true;
      retireRefNextFrame(gen, false);
      return;
    }
    fadeOutNavFocus();
  });

  const holdTimer = setTimeout(
    () => {
      if (_generation !== gen) return;
      _holdOver = true;
      if (_dismissQueued) fadeOutNavFocus();
    },
    scaledDurationMs(NAV_FOCUS_RAMP_MS) + NAV_FOCUS_HOLD_MS,
  );

  _teardown = () => {
    removeListeners();
    clearTimeout(holdTimer);
  };
}

function fadeOutNavFocus(): void {
  const el = _markedEl;
  if (!el) return;
  _teardown?.();
  _teardown = null;

  const reduced = isReducedMotion();
  if (reduced) {
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
  } else {
    el.classList.add(NAV_FOCUS_FADING_CLASS);
  }

  const gen = _generation;
  retireRefNextFrame(gen, reduced);

  if (reduced) return;

  const fadeTimer = setTimeout(() => {
    if (_generation !== gen) return;
    el.classList.remove(NAV_FOCUS_STUCK_CLASS);
    el.classList.remove(NAV_FOCUS_FADING_CLASS);
    _markedEl = null;
    _refLive = false;
    _teardown = null;
  }, scaledDurationMs(NAV_FOCUS_FADE_MS));
  _teardown = () => clearTimeout(fadeTimer);
}

export function clearNavFocus(): void {
  if (_teardown) {
    _teardown();
    _teardown = null;
  }
  _cancelRefExpiry?.();
  _cancelRefExpiry = null;
  if (_markedEl) {
    _markedEl.classList.remove(NAV_FOCUS_STUCK_CLASS);
    _markedEl.classList.remove(NAV_FOCUS_FADING_CLASS);
    _markedEl = null;
  }
  _refLive = false;
  _generation++;
}
