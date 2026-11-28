import { useEffect } from 'preact/hooks';
import { markAnchorScroll } from '../components/chat/scrollState';
import { getRemPx, opensSoftwareKeyboard, viewportIsKeyboardShrunk } from '../utils/dom';
import { nearestScrollableAncestor, revealScrollTop } from '../utils/focusReveal';
import { nowMs } from '../utils/scrollActivity';
import { postClientLog } from '../utils/clientLog';
import { readViewport } from '../components/chat/probeViewport';

const REVEAL_MARGIN_REM = 1;

const KEYBOARD_ACCESSORY_PX = 64;

const SETTLE_MS = 600;

export function installFocusedFieldVisible(): () => void {
  let armed: HTMLElement | null = null;
  let scroller: HTMLElement | null = null;
  let pending: number | null = null;
  let settleUntil = 0;
  let lastSeen: Record<string, unknown> | null = null;

  function reveal() {
    pending = null;
    const field = armed;
    if (!field || document.activeElement !== field) return;
    if (!scroller || !scroller.contains(field)) scroller = nearestScrollableAncestor(field);
    const container = scroller;
    if (!container) return;
    const viewTop = container.getBoundingClientRect().top + container.clientTop;
    const rect = field.getBoundingClientRect();
    const marginTop = REVEAL_MARGIN_REM * getRemPx();
    const vv = window.visualViewport;
    const keyboardUp = !!vv && viewportIsKeyboardShrunk(vv.height, window.innerHeight);
    const next = revealScrollTop({
      scrollTop: container.scrollTop,
      viewTop,
      viewBottom: viewTop + container.clientHeight,
      fieldTop: rect.top,
      fieldBottom: rect.bottom,
      marginTop,
      marginBottom: marginTop + (keyboardUp ? KEYBOARD_ACCESSORY_PX : 0),
    });
    lastSeen = {
      field: field.tagName.toLowerCase(),
      viewTop: Math.round(viewTop),
      viewH: container.clientHeight,
      fieldTop: Math.round(rect.top),
      fieldBottom: Math.round(rect.bottom),
      scrollTop: Math.round(container.scrollTop),
      maxScroll: Math.round(container.scrollHeight - container.clientHeight),
      band: document.documentElement.style.getPropertyValue('--keyboard-band'),
      keyboardUp,
      want: next === null ? null : Math.round(next),
    };
    if (next === null) return;
    markAnchorScroll(container, next);
  }

  function tick() {
    reveal();
    if (armed && nowMs() < settleUntil) schedule();
    else reportEpisode();
  }

  /** One breadcrumb per keyboard open, at the end of the settle window.
   *
   *  A diagnostic, registered in `docs/temporary-measures.md`. This behaviour
   *  cannot be reproduced off-device, and three rounds of theoretical fixes
   *  failed, so the device reports the numbers instead. It lands in engine.log
   *  as `[Client/mobile] focus-reveal`. */
  function reportEpisode() {
    const seen = lastSeen;
    lastSeen = null;
    if (!seen || !scroller) return;
    postClientLog('mobile', 'focus-reveal', {
      ...readViewport(),
      ...seen,
      landed: Math.round(scroller.scrollTop),
    });
  }

  function schedule() {
    if (!armed || pending !== null) return;
    pending = requestAnimationFrame(tick);
  }

  function settle() {
    if (!armed) return;
    settleUntil = nowMs() + SETTLE_MS;
    schedule();
  }

  function disarm() {
    armed = null;
    scroller = null;
    settleUntil = 0;
    lastSeen = null;
    if (pending === null) return;
    cancelAnimationFrame(pending);
    pending = null;
  }

  function onFocusIn(e: FocusEvent) {
    if (!opensSoftwareKeyboard(e.target)) {
      disarm();
      return;
    }
    disarm();
    armed = e.target as HTMLElement;
    settle();
  }

  function onFocusOut(e: FocusEvent) {
    if (e.target === armed) disarm();
  }

  document.addEventListener('focusin', onFocusIn, { passive: true });
  document.addEventListener('focusout', onFocusOut, { passive: true });
  document.addEventListener('touchmove', disarm, { passive: true, capture: true });
  window.visualViewport?.addEventListener('resize', settle);

  return () => {
    document.removeEventListener('focusin', onFocusIn);
    document.removeEventListener('focusout', onFocusOut);
    document.removeEventListener('touchmove', disarm, { capture: true });
    window.visualViewport?.removeEventListener('resize', settle);
    disarm();
  };
}

export function useFocusedFieldVisible() {
  useEffect(installFocusedFieldVisible, []);
}
