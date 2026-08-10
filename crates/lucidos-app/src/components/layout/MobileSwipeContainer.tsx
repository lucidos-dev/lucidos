import { useRef, useCallback, useEffect, useLayoutEffect } from 'preact/hooks';
import { mobileView, MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, appPseudoFullscreen } from '../../store/store';
import { navigateToPane, resolveSwipePane } from '../../store/actions/pane';
import { MOBILE_PANE_CONFIGS } from './MobileAppHeader';
import { EdgeSwipeZones } from './EdgeSwipeZones';
import { isTextInput, isInteractiveTarget, opensSoftwareKeyboard } from '../../utils/dom';
import { SwipeTouch } from '../../utils/swipe';

export { SwipeTouch } from '../../utils/swipe';

// Rubber band factor at edges (0 = no movement, 1 = full movement)
const RUBBER_BAND = 0.3;

/** Width of the LEFT screen-edge strip (px) treated as a potential iOS *back*
 *  navigation swipe and suppressed. It MUST cover the full `.edge-swipe-left`
 *  zone (2.5rem ≈ 40px in mobile.css): over an app iframe in the content pane,
 *  the ONLY place a pane swipe can begin is that edge zone (the iframe captures
 *  every other touch), so an app→thread back-swipe is forced to start there
 *  every time. With the old 24px guard, a swipe beginning in the 24–40px band
 *  reached the in-app handler but did NOT preventDefault — so WebKit's native
 *  pop gesture fired and the PWA navigated out to the workspace gateway picker.
 *  Matching the zone width closes that band. iOS's interactive pop activates
 *  from roughly the outermost ~20pt, well within 40px. */
export const EDGE_NAV_GUARD_LEFT_PX = 40;

/** Width of the RIGHT screen-edge strip — the *forward* navigation gesture.
 *  Covers the `.edge-swipe-right` zone (1.25rem ≈ 20px) with a little margin.
 *  Kept narrower than the left strip so it stays clear of content/scrolling. */
export const EDGE_NAV_GUARD_RIGHT_PX = 24;

/** Pure decision: should a touchstart at `clientX` call preventDefault() to
 *  suppress iOS's native back/forward navigation swipe?
 *
 *  A standalone iOS PWA exposes NO CSS/touch-action opt-out for this gesture,
 *  and WebKit's edge recognizer commits before our in-app 8px horizontal lock
 *  (SwipeTouch) — so preventing the default in onTouchMove runs too late. The
 *  only reliable suppression is preventDefault on the touchstart itself. Scoped
 *  to the screen-edge strips (sized to the `.edge-swipe-*` zones) and to
 *  non-interactive, non-text-input targets so taps on edge controls and
 *  vertical scrolling elsewhere survive. */
export function shouldSuppressEdgeNavigation(args: {
  clientX: number;
  viewportWidth: number;
  targetIsInteractive: boolean;
  textInputFocused: boolean;
}): boolean {
  const { clientX, viewportWidth, targetIsInteractive, textInputFocused } = args;
  if (targetIsInteractive || textInputFocused) return false;
  return clientX <= EDGE_NAV_GUARD_LEFT_PX || clientX >= viewportWidth - EDGE_NAV_GUARD_RIGHT_PX;
}

/** Pure decision: may a Lucidos pane swipe START for this touch?
 *
 *  The first two are properties of the touch: a focused text input means the
 *  user is typing and a horizontal drag must not navigate away, and a
 *  horizontally-scrollable target (a code block, a range slider knob) owns its
 *  own horizontal drag.
 *
 *  `appFullscreen` is the odd one out, being a property of the app rather than
 *  the touch: a pseudo-fullscreen app IS the screen, so the three-pane swipe is
 *  off entirely. Nothing visible moves during it anyway (the overlay is
 *  position:fixed over the viewport and the track's transform is pinned to
 *  `none`), so a pane change there is invisible state drift, and the user
 *  leaves fullscreen on a pane they never chose. Deliberately does NOT cover
 *  the separate suppression of WebKit's native edge gesture, which must keep
 *  running while fullscreen: see `shouldSuppressEdgeNavigation`. */
export function shouldStartPaneSwipe(args: {
  textInputFocused: boolean;
  targetScrollable: boolean;
  appFullscreen: boolean;
}): boolean {
  const { textInputFocused, targetScrollable, appFullscreen } = args;
  return !textInputFocused && !targetScrollable && !appFullscreen;
}

/** Pure decision: does focus on `el` mean the mobile keyboard is up over the
 *  app, i.e. should `data-keyboard-active` be set?
 *
 *  Scoped to `<textarea>` (not every text input) so the header search bar stays
 *  interactive while its `<input>` is focused. The thread-title editor is
 *  excluded because its own container (`.mobile-thread-title-row`) is one of the
 *  things the flag inerts, which would lock the user out of the editor they just
 *  opened and defeat tap-outside-to-blur.
 *
 *  Takes the element rather than reading `document.activeElement` itself, so the
 *  same predicate answers for a focus EVENT's target and for the live focus (see
 *  `reconcileKeyboardActive`). Duck-typed on `tagName` + `closest` rather than
 *  `instanceof`, matching `shouldSuppressDragStart`: testable without a DOM, and
 *  realm-agnostic. */
export function isKeyboardActiveTarget(el: EventTarget | null): boolean {
  const node = el as { tagName?: string; closest?: (sel: string) => unknown } | null;
  if (!node || node.tagName !== 'TEXTAREA' || typeof node.closest !== 'function') return false;
  return node.closest('.mobile-thread-title-row') == null;
}

/** Attribute sink: `<html>` in the app, a recording stub in tests. */
interface AttrTarget {
  setAttribute(name: string, value: string): void;
  removeAttribute(name: string): void;
}

/** Re-derive `data-keyboard-active` from the LIVE focus, rather than from the
 *  last focus event seen.
 *
 *  The focusin/focusout pair below describes focus by its TRANSITIONS, and one
 *  transition is never reported: removing the focused element from the DOM moves
 *  focus to `<body>` WITHOUT firing focusout, in WebKit and Chromium alike
 *  (verified on mobile WebKit). The flag then outlives the keyboard that
 *  justified it, and everything it gates stays inert with nothing focused and no
 *  event left to clear it. An iOS PWA resume is the same hazard from the other
 *  side: the page can come back with the attribute intact and focus dropped.
 *
 *  Idempotent, so it is safe to run on every touch. */
export function reconcileKeyboardActive(root: AttrTarget, active: EventTarget | null): void {
  if (isKeyboardActiveTarget(active)) root.setAttribute('data-keyboard-active', '');
  else root.removeAttribute('data-keyboard-active');
}

/** Pure decision: what value should `--app-height` be written to, given the
 *  current visual viewport, layout viewport, and focus state? Extracted so
 *  the wake/resize bug can be unit-tested without jsdom + fake visualViewport.
 *
 *  Keyboard up iff `vv.height` is meaningfully shrunk relative to
 *  `window.innerHeight` (the layout viewport, which doesn't shrink for the
 *  iOS keyboard) AND a text input that triggers the OS keyboard is focused.
 *  When keyboard is up, render the app shell at `vv.height` (shrunk to fit
 *  above the keyboard). Otherwise use `innerHeight` as the truth — this is
 *  what fixes the iOS PWA wake bug, where a delayed `vv.resize` fires with
 *  a stale shrunk `vv.height` even though the keyboard is actually dismissed.
 *  The companion fix is in `onWake`, which blurs any iOS-preserved focus on
 *  a text input so the `activeElementOpensKeyboard` signal correctly reports
 *  false when the keyboard is actually down. */
export function computeAppHeight(args: {
  vvHeight: number;
  innerHeight: number;
  activeElementOpensKeyboard: boolean;
}): number {
  const { vvHeight, innerHeight, activeElementOpensKeyboard } = args;
  const isKeyboard = vvHeight < innerHeight - 100 && activeElementOpensKeyboard;
  return isKeyboard ? vvHeight : innerHeight;
}

/** Check if an element or any ancestor (up to pane boundary) scrolls horizontally. */
function isHorizontallyScrollable(el: Element | null): boolean {
  while (el) {
    if (el.classList.contains('mobile-swipe-pane')) break;
    if (el.scrollWidth > el.clientWidth) {
      const style = getComputedStyle(el);
      if (style.overflowX === 'auto' || style.overflowX === 'scroll') return true;
    }
    el = el.parentElement;
  }
  return false;
}

/** Convert a pane index to a CSS percentage translateX value.
 *  Uses the track's own width (300% of container = 3 × paneWidth).
 *  Pane 0 → 0%, Pane 1 → −33.333%, Pane 2 → −66.667%. */
function paneTransform(index: number): string {
  return `translateX(${-index * 100 / PANE_COUNT}%)`;
}

/** Mobile-only swipeable container with three full-screen views.
 *
 *  Architecture: CSS transform (not scroll) — mobileView signal is the
 *  single source of truth for both pane position AND header display.
 *  The header reads mobileView to show/hide sections via CSS.
 *  The track reads mobileView to position panes via useLayoutEffect
 *  (runs BEFORE paint — no frame where header and pane disagree).
 *
 *  Desync prevention:
 *  1. All pane navigation goes through navigateToPane() which atomically
 *     closes drawers + updates the signal.
 *  2. useLayoutEffect (not useEffect) derives CSS transform from signal
 *     before the browser paints — header/dots and pane move in the same frame.
 *  3. transitionend handler reconciles transform as a safety net.
 *  4. SwipeTouch is pure (no DOM state) — only returns deltas. */
export function MobileSwipeContainer() {
  const containerRef = useRef<HTMLDivElement>(null);
  const trackRef = useRef<HTMLDivElement>(null);
  const touch = useRef(new SwipeTouch());
  const mountedRef = useRef(false);

  // Must be useLayoutEffect — see component JSDoc.
  useLayoutEffect(() => {
    const track = trackRef.current;
    if (!track) return;

    const container = containerRef.current;
    const paneWidth = container?.offsetWidth ?? 0;
    const index = PANE_INDEX[mobileView.value];

    if (!mountedRef.current) {
      // First render: position without animation.
      mountedRef.current = true;
      track.style.transition = 'none';
    } else {
      // Subsequent renders: re-enable CSS transition for smooth animation.
      track.style.transition = '';
    }

    if (paneWidth > 0) {
      track.style.transform = `translateX(${-index * paneWidth}px)`;
    } else {
      track.style.transform = paneTransform(index);
    }
  }, [mobileView.value]);

  // Safety net: after every CSS transition on the track ends, verify the
  // transform matches the mobileView signal. If something caused them to
  // disagree (resize during animation, interrupted transition, browser quirk),
  // this corrects it without a visible jump.
  useEffect(() => {
    const track = trackRef.current;
    const container = containerRef.current;
    if (!track || !container) return;
    const onTransitionEnd = (e: TransitionEvent) => {
      if (e.target !== track || e.propertyName !== 'transform') return;
      const paneWidth = container.offsetWidth;
      if (paneWidth <= 0) return;
      const correctValue = `translateX(${-PANE_INDEX[mobileView.value] * paneWidth}px)`;
      if (track.style.transform !== correctValue) {
        track.style.transform = correctValue;
      }
    };
    track.addEventListener('transitionend', onTransitionEnd);
    return () => track.removeEventListener('transitionend', onTransitionEnd);
  }, []);

  // Handle resize: snap to current pane without animation.
  // Guards against height-only changes (e.g., iOS keyboard) to avoid
  // unnecessary transition disable/re-enable on every keyboard toggle.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    let lastPaneWidth = 0;
    const observer = new ResizeObserver(() => {
      const track = trackRef.current;
      if (!track) return;
      const paneWidth = container.offsetWidth;
      if (paneWidth === 0 || paneWidth === lastPaneWidth) return;
      lastPaneWidth = paneWidth;
      track.style.transition = 'none';
      track.style.transform = `translateX(${-PANE_INDEX[mobileView.value] * paneWidth}px)`;
      requestAnimationFrame(() => {
        if (trackRef.current) trackRef.current.style.transition = '';
      });
    });
    observer.observe(container);
    return () => observer.disconnect();
  }, []);

  // Suppress swipe track transition when exiting pseudo-fullscreen.
  // The CSS override (transform: none !important) drops, which would
  // animate the track back to its pane position.
  const wasPseudoFullscreen = useRef(false);
  useLayoutEffect(() => {
    const isPseudo = appPseudoFullscreen.value;
    if (wasPseudoFullscreen.current && !isPseudo) {
      const track = trackRef.current;
      if (track) {
        track.style.transition = 'none';
        requestAnimationFrame(() => { if (trackRef.current) trackRef.current.style.transition = ''; });
      }
    }
    wasPseudoFullscreen.current = isPseudo;
  }, [appPseudoFullscreen.value]);

  // ── Touch event handlers ───────────────────────────────────────────────

  const touchTargetScrollable = useRef(false);

  const onTouchStart = useCallback((e: TouchEvent) => {
    const t = e.touches[0];
    const target = e.target as Element;

    const textInputFocused = isTextInput(document.activeElement);
    // Don't hijack touches on horizontally-scrollable children (e.g., code blocks)
    // or range sliders (knob drag is horizontal and must not trigger pane swipe).
    touchTargetScrollable.current = isHorizontallyScrollable(target) ||
      !!target.closest('input[type="range"]');

    // Suppress iOS's native back/forward navigation swipe at the screen edges.
    // Must happen here on touchstart — the listener is registered { passive:
    // false } for this — because WebKit's edge recognizer commits before our
    // 8px horizontal lock, so onTouchMove's preventDefault is too late. Exempts
    // scrollable children, interactive controls, and focused text inputs so
    // their own gestures/taps survive (see shouldSuppressEdgeNavigation).
    if (t && !touchTargetScrollable.current && shouldSuppressEdgeNavigation({
      clientX: t.clientX,
      viewportWidth: window.innerWidth,
      targetIsInteractive: isInteractiveTarget(target),
      textInputFocused,
    })) {
      e.preventDefault();
    }

    // Must come AFTER the suppression above: a pseudo-fullscreen app cancels
    // OUR swipe, but WebKit's native one still has to be preventDefault'd or
    // suppressing ours just hands the gesture over. See shouldStartPaneSwipe.
    if (!shouldStartPaneSwipe({
      textInputFocused,
      targetScrollable: touchTargetScrollable.current,
      appFullscreen: appPseudoFullscreen.value,
    })) return;

    touch.current.start(t.clientX, t.clientY);
    const track = trackRef.current;
    if (track) track.style.transition = 'none';
  }, []);

  const onTouchMove = useCallback((e: TouchEvent) => {
    if (touchTargetScrollable.current) return;

    const t = e.touches[0];
    const dx = touch.current.move(t.clientX, t.clientY);
    if (dx === null) return;

    e.preventDefault();

    const container = containerRef.current;
    const track = trackRef.current;
    if (!container || !track) return;

    const paneWidth = container.offsetWidth;
    const baseOffset = -PANE_INDEX[mobileView.value] * paneWidth;
    let offset = baseOffset + dx;

    // Rubber band at edges
    const minOffset = -(PANE_COUNT - 1) * paneWidth;
    if (offset > 0) {
      offset = offset * RUBBER_BAND;
    } else if (offset < minOffset) {
      offset = minOffset + (offset - minOffset) * RUBBER_BAND;
    }

    track.style.transform = `translateX(${offset}px)`;
  }, []);

  const onTouchEnd = useCallback(() => {
    if (touchTargetScrollable.current) return;

    const container = containerRef.current;
    const track = trackRef.current;
    if (!container || !track) return;

    const paneWidth = container.offsetWidth;
    const paneDelta = touch.current.end(paneWidth);
    const target = resolveSwipePane(paneDelta);

    if (target) {
      // Pane change: navigateToPane updates the signal, useLayoutEffect
      // handles the transform + transition re-enable before paint.
      navigateToPane(target);
    } else {
      // Snap back: signal unchanged, handle transform directly.
      track.style.transition = '';
      track.style.transform = `translateX(${-PANE_INDEX[mobileView.value] * paneWidth}px)`;
    }
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    // touchstart is non-passive: onTouchStart calls preventDefault() at the
    // screen edges to suppress iOS's native back/forward navigation swipe.
    el.addEventListener('touchstart', onTouchStart, { passive: false });
    el.addEventListener('touchmove', onTouchMove, { passive: false });
    el.addEventListener('touchend', onTouchEnd, { passive: true });
    el.addEventListener('touchcancel', onTouchEnd, { passive: true });
    return () => {
      el.removeEventListener('touchstart', onTouchStart);
      el.removeEventListener('touchmove', onTouchMove);
      el.removeEventListener('touchend', onTouchEnd);
      el.removeEventListener('touchcancel', onTouchEnd);
    };
  }, [onTouchStart, onTouchMove, onTouchEnd]);

  // ── Keyboard-active: block accidental taps on other elements ─────────

  // When a textarea is focused on mobile, set data-keyboard-active on
  // <html> so CSS can disable pointer-events on non-textarea elements.
  useEffect(() => {
    const root = document.documentElement;
    const onFocusIn = (e: FocusEvent) => {
      if (isKeyboardActiveTarget(e.target)) root.setAttribute('data-keyboard-active', '');
    };
    const onFocusOut = (e: FocusEvent) => {
      // relatedTarget is the next focused element — if it's also a triggering
      // textarea, keep the attribute (user tapped between prompt fields).
      if (!isKeyboardActiveTarget(e.relatedTarget)) root.removeAttribute('data-keyboard-active');
    };
    // Sweep the flag back into agreement with the live focus at the moments a
    // stale one would first be FELT: the user's next touch (capture phase, so an
    // inert target still reports it) and a return to the foreground. See
    // `reconcileKeyboardActive` for the transition that fires no event at all.
    const reconcile = () => reconcileKeyboardActive(root, document.activeElement);
    document.addEventListener('focusin', onFocusIn, { passive: true });
    document.addEventListener('focusout', onFocusOut, { passive: true });
    document.addEventListener('touchstart', reconcile, { passive: true, capture: true });
    document.addEventListener('visibilitychange', reconcile, { passive: true });
    window.addEventListener('pageshow', reconcile, { passive: true });
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
      document.removeEventListener('touchstart', reconcile, { capture: true });
      document.removeEventListener('visibilitychange', reconcile);
      window.removeEventListener('pageshow', reconcile);
      root.removeAttribute('data-keyboard-active');
    };
  }, []);

  // ── iOS Safari workarounds ─────────────────────────────────────────────

  // iOS Safari can scroll the document (window.scrollY > 0) when the
  // keyboard opens or during certain touch interactions.
  useEffect(() => {
    const onWindowScroll = () => {
      if (window.scrollY !== 0) window.scrollTo(0, 0);
    };
    window.addEventListener('scroll', onWindowScroll, { passive: true });
    return () => window.removeEventListener('scroll', onWindowScroll);
  }, []);

  // iOS Safari auto-scrolls overflow:hidden containers when focus() targets
  // an offscreen element (e.g. prompt input on pane 1 while pane 0 is visible).
  // This sets container.scrollLeft to a non-zero value, permanently offsetting
  // the view from the CSS transform position. Reset it immediately.
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    const onContainerScroll = () => {
      if (container.scrollLeft !== 0) container.scrollLeft = 0;
    };
    container.addEventListener('scroll', onContainerScroll, { passive: true });
    return () => container.removeEventListener('scroll', onContainerScroll);
  }, []);

  // Track visual viewport for iOS keyboard handling (--app-height).
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return;
    // -1 sentinel ensures the initial setHeight() call writes the CSS variable
    // instead of being short-circuited by the equality guard inside setHeight.
    let lastSetHeight = -1;

    // px, not rem: --app-height is a physical viewport measurement that must
    // NOT scale with --user-ui-scale. Using rem caused the app shell to exceed
    // the viewport when applyUiScale() changed the base font-size after mount.
    const setHeight = (h: number) => {
      if (h === lastSetHeight) return;
      lastSetHeight = h;
      document.documentElement.style.setProperty('--app-height', `${h}px`);
    };

    // One computation site so onResize, onOrientationChange, the initial
    // write, and the wake-time recompute all use the same keyboard-aware
    // decision. Routing onOrientationChange / initial through this fixes the
    // case where the user has the keyboard up at rotation or component mount
    // — writing innerHeight blindly would stamp the full layout viewport
    // over the keyboard-shrunk visual viewport, occluding the prompt area
    // behind the keyboard until the next vv.resize repaired it.
    const currentAppHeight = () => computeAppHeight({
      vvHeight: vv.height,
      innerHeight: window.innerHeight,
      activeElementOpensKeyboard: opensSoftwareKeyboard(document.activeElement),
    });

    const onResize = () => {
      // The keyboard opening or closing changes --app-height, which shortens or
      // lengthens the transcript's viewport. The reader is left exactly where
      // they are: the content above them has not moved, so the same scrollTop
      // still shows them the same thing. (This used to re-pin a bottom reader
      // to the new bottom; see scrollState's header for why nothing does that
      // any more.)
      setHeight(currentAppHeight());
    };
    const onOrientationChange = () => {
      setHeight(currentAppHeight());
    };
    // iOS PWA suspend/resume often dismisses the on-screen keyboard without
    // firing a visualViewport `resize` event. Without a wake-time recompute,
    // --app-height stays at the keyboard-shrunk value and .app-shell renders
    // at half the viewport (visible black band below the prompt) until reload.
    //
    // Two things both go wrong on iOS PWA wake and BOTH need fixing — the
    // prior fix only covered (1):
    //
    //   1. window.visualViewport.height is often pinned at the stale shrunk
    //      value at wake time, so going through onResize() (which used to
    //      trust vv.height) would re-stamp the small value. window.innerHeight
    //      is the layout viewport — doesn't shrink for the iOS keyboard and
    //      is reliably restored on resume — so use it as the truth on wake.
    //
    //   2. iOS preserves focus on the textarea across suspend even when the
    //      on-screen keyboard is dismissed. Ghost-focused state then fools
    //      computeAppHeight's keyboard check (vv.height < innerHeight - 100
    //      AND opensSoftwareKeyboard(activeElement) = both true) when a
    //      delayed vv.resize finally fires — the helper returns vv.height
    //      and onResize writes the stale shrunk value back into --app-height.
    //      Explicit blur clears the ghost focus; the user re-taps to reopen
    //      the keyboard naturally, matching the actual on-screen state.
    //
    // The blur is scoped to the case where vv.height appears shrunk relative
    // to the layout viewport — that's the only scenario where keyboard-related
    // ghost focus matters. Without the scope, a quick tab-switch return while
    // a search modal or settings input is focused would dismiss its focus
    // for no benefit (no keyboard-related state to clear).
    //
    // Reset lastSetHeight so the CSS write happens even when innerHeight
    // matches the cached value.
    const onWake = () => {
      const vvLooksShrunk = vv.height < window.innerHeight - 100;
      const active = document.activeElement;
      if (vvLooksShrunk && active instanceof HTMLElement && opensSoftwareKeyboard(active)) {
        active.blur();
      }
      lastSetHeight = -1;
      setHeight(currentAppHeight());
    };
    const onVisibilityChange = () => {
      if (document.visibilityState === 'visible') onWake();
    };
    vv.addEventListener('resize', onResize);
    window.addEventListener('orientationchange', onOrientationChange);
    document.addEventListener('visibilitychange', onVisibilityChange);
    window.addEventListener('pageshow', onWake);
    setHeight(currentAppHeight());
    return () => {
      vv.removeEventListener('resize', onResize);
      window.removeEventListener('orientationchange', onOrientationChange);
      document.removeEventListener('visibilitychange', onVisibilityChange);
      window.removeEventListener('pageshow', onWake);
      document.documentElement.style.removeProperty('--app-height');
    };
  }, []);

  return (
    <div class="mobile-swipe-wrapper">
      <div ref={containerRef} class="mobile-swipe-container">
        <div ref={trackRef} class="mobile-swipe-track">
          {MOBILE_VIEWS.map((v) => {
            const { Pane } = MOBILE_PANE_CONFIGS[v];
            return (
              <div key={v} class="mobile-swipe-pane">
                <Pane />
                {/* Rendered inside each pane (not the swipe container) so they
                    share a stacking context with the prompt-area, allowing
                    .prompt-area's z-index:2 to keep its buttons clickable. */}
                <EdgeSwipeZones />
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
