import { useEffect } from 'preact/hooks';
import { markAnchorScroll } from '../components/chat/scrollState';
import { getRemPx, opensSoftwareKeyboard, viewportIsKeyboardShrunk } from '../utils/dom';
import { nearestScrollableAncestor, revealScrollTop } from '../utils/focusReveal';
import { nowMs } from '../utils/scrollActivity';
import { postClientLog } from '../utils/clientLog';
import { readViewport } from '../components/chat/probeViewport';

/** Clear space left between the focused field and the nearer viewport edge.
 *  A field flush against the keyboard reads as clipped. */
const REVEAL_MARGIN_REM = 1;

/** The strip at the foot of the visual viewport that iOS draws its keyboard
 *  accessory bar over: the prev / next / Done control.
 *
 *  `visualViewport.height` stops at the top of the KEYS, and since iOS 26 that
 *  bar is a capsule FLOATING above them rather than a docked row. So it covers
 *  web content the viewport still calls visible, and the reveal parked the
 *  field right under it.
 *
 *  A constant because nothing measures it. No API reports the bar, and it is in
 *  no safe-area inset. Measured off a reported screenshot at roughly 60px, then
 *  rounded up: over-reserving costs a field sitting a little high, and
 *  under-reserving is the bug.
 *
 *  px rather than rem, like `--app-height`: it is OS chrome at a fixed CSS size
 *  and does not scale with `--user-ui-scale`. */
const KEYBOARD_ACCESSORY_PX = 64;

/** How long the reveal keeps re-placing the field after the last viewport
 *  change, in ms.
 *
 *  One measurement per resize is not enough, because ours is not the last
 *  write. WebKit runs its OWN scroll-into-view for the focused field as the
 *  keyboard settles. It aims at a clearance of a few pixels, and whichever of
 *  us writes last wins. That race is what made the reveal land differently on
 *  the same form twice running.
 *
 *  So the reveal re-measures every frame until the viewport has been quiet for
 *  this long. It is idempotent, so the extra frames cost a measurement and
 *  write nothing once the field is placed. Comfortably past both the keyboard
 *  animation and WebKit's correction. */
const SETTLE_MS = 600;

/** Keep the focused field visible when the mobile keyboard changes the
 *  viewport: the *focused-field reveal* (`docs/glossary.md`).
 *
 *  The browser scrolls a field into view at FOCUS time, against the viewport it
 *  can see then. The iOS keyboard shrinks the viewport after that, and
 *  `MobileSwipeContainer` follows by shrinking `.app-shell` through
 *  `--app-height`. The pane's scroll container shrinks with it, so the field
 *  the user just tapped ends up behind the keyboard with nothing left to
 *  re-run.
 *
 *  So the trigger is `visualViewport.resize`, not focus: only then is the
 *  container's height the one the field has to fit. A frame after `focusin`
 *  covers the other case, a lower field tapped with the keyboard already up.
 *
 *  Mobile-only by its mount, and it changes no height in the shell's chain.
 *  Both, with the reported form, in
 *  `docs/plans/2026-09-19-a-focused-field-stays-above-the-keyboard.md`.
 *
 *  A plain function returning its teardown, so the behaviour can be driven in
 *  a test without mounting a component. */
export function installFocusedFieldVisible(): () => void {
  /** The field whose reveal window is open, or null when none is. */
  let armed: HTMLElement | null = null;
  /** Its scroll container, resolved on the first frame that finds one. A form
   *  overflows only once the keyboard shrinks the pane, so the lookup retries
   *  until it succeeds, then stops walking the tree every frame. */
  let scroller: HTMLElement | null = null;
  let pending: number | null = null;
  /** When the settle window closes, in `nowMs()` terms. */
  let settleUntil = 0;
  /** What the last frame of the episode saw, for the probe to report. */
  let lastSeen: Record<string, unknown> | null = null;

  function reveal() {
    pending = null;
    const field = armed;
    if (!field || document.activeElement !== field) return;
    // Re-resolve when the cached one no longer holds the field: a form can
    // re-render its scroll region under a focus the browser kept.
    if (!scroller || !scroller.contains(field)) scroller = nearestScrollableAncestor(field);
    const container = scroller;
    if (!container) return;
    // The CLIENT box, so a border is discounted. `clientHeight` is also what
    // shrinks when the keyboard does: the shell's height reaches the container
    // through the pane's flex column.
    const viewTop = container.getBoundingClientRect().top + container.clientTop;
    const rect = field.getBoundingClientRect();
    const marginTop = REVEAL_MARGIN_REM * getRemPx();
    const vv = window.visualViewport;
    // The accessory bar exists only while the keys are on screen, so the strip
    // is reserved only then. Charging it at focus time, before the viewport has
    // shrunk, would move the field twice for one tap.
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
    // An ANCHOR write, not a placement: the layout moved under a reader who did
    // not scroll. Unmarked, the mobile hide-on-scroll header spends the delta
    // as sliding chrome. The transcript's render window reads one near the top
    // as a request for older turns. See `markAnchorScroll`.
    //
    // A target past the end of the range is re-asked for every frame of the
    // window, which is deliberate: the DOM clamps it, an unchanged `scrollTop`
    // fires no scroll event, and the next frame is what notices the range
    // growing. Remembering it instead would skip that.
    markAnchorScroll(container, next);
  }

  /** Measure on the next frame, and keep measuring until the settle window
   *  closes. The frame also puts the read after `--app-height` has been
   *  written, whichever order the two resize listeners happen to run in. */
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

  /** Reopen the settle window and make sure a frame is queued. Every trigger
   *  goes through here, so a burst of resize events during the keyboard
   *  animation extends one window rather than starting several. */
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

  // A finger drag hands the container back to the reader, so the window closes
  // until the next focus. Without it a late resize would yank them back to the
  // field. Any `scrollTop` write during a fling also cancels iOS momentum
  // outright (`utils/scrollActivity.ts`). Capture phase, so an inert or
  // stopped-propagation subtree still reports the drag.
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

/** Mount the focused-field reveal for the lifetime of the calling component. */
export function useFocusedFieldVisible() {
  useEffect(installFocusedFieldVisible, []);
}
