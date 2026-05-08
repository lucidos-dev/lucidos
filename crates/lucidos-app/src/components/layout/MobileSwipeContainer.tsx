import { useRef, useCallback, useEffect, useLayoutEffect } from 'preact/hooks';
import { mobileView, MOBILE_VIEWS, PANE_INDEX, PANE_COUNT, appPseudoFullscreen } from '../../store/store';
import { navigateToPane, resolveSwipePane } from '../../store/actions/pane';
import { MOBILE_PANE_CONFIGS } from './MobileAppHeader';
import { isTextInput } from '../../utils/dom';
import { SwipeTouch } from '../../utils/swipe';
import { scrolledUp, scrollToBottom } from '../chat/scrollState';

export { SwipeTouch } from '../../utils/swipe';

// Rubber band factor at edges (0 = no movement, 1 = full movement)
const RUBBER_BAND = 0.3;

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
    // Don't start pane swipes while a text input is focused — the user is
    // typing and horizontal drags should not navigate away.
    if (isTextInput(document.activeElement)) return;

    const target = e.target as Element;
    // Don't hijack touches on horizontally-scrollable children (e.g., code blocks)
    // or range sliders (knob drag is horizontal and must not trigger pane swipe).
    touchTargetScrollable.current = isHorizontallyScrollable(target) ||
      !!target.closest('input[type="range"]');
    if (touchTargetScrollable.current) return;

    const t = e.touches[0];
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
    el.addEventListener('touchstart', onTouchStart, { passive: true });
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
  // Scoped to <textarea> (not all text inputs) so the header search bar
  // remains interactive when its <input> is focused. Excludes the title
  // editor — its container (.mobile-thread-title-row) is one of the
  // pointer-events targets, which would lock the user out of their own
  // editor and prevent tap-outside-to-blur.
  useEffect(() => {
    const root = document.documentElement;
    const triggers = (el: EventTarget | null) =>
      el instanceof HTMLTextAreaElement && !el.closest('.mobile-thread-title-row');
    const onFocusIn = (e: FocusEvent) => {
      if (triggers(e.target)) root.setAttribute('data-keyboard-active', '');
    };
    const onFocusOut = (e: FocusEvent) => {
      // relatedTarget is the next focused element — if it's also a triggering
      // textarea, keep the attribute (user tapped between prompt fields).
      if (!triggers(e.relatedTarget)) root.removeAttribute('data-keyboard-active');
    };
    document.addEventListener('focusin', onFocusIn, { passive: true });
    document.addEventListener('focusout', onFocusOut, { passive: true });
    return () => {
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
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
    let fullHeight = vv.height;
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

    const onResize = () => {
      // Capture before layout changes — once --app-height updates and
      // ResizeObserver fires, scrolledUp may flip to true.
      const wasAtBottom = !scrolledUp.value;
      const isKeyboard = vv.height < fullHeight - 100 &&
        isTextInput(document.activeElement);
      if (!isKeyboard) {
        fullHeight = vv.height;
      }
      setHeight(vv.height);
      // scrollToBottom() sets ResizeObserver suppression to 'scroll',
      // so the observer scrolls to bottom instead of marking scrolledUp.
      if (wasAtBottom) {
        scrollToBottom();
      }
    };
    const onOrientationChange = () => {
      fullHeight = vv.height;
      setHeight(fullHeight);
    };
    vv.addEventListener('resize', onResize);
    window.addEventListener('orientationchange', onOrientationChange);
    setHeight(fullHeight);
    return () => {
      vv.removeEventListener('resize', onResize);
      window.removeEventListener('orientationchange', onOrientationChange);
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
                {/* Edge swipe zones — see .edge-swipe-zone in mobile.css.
                    Rendered inside each pane (not the swipe container) so they
                    share a stacking context with the prompt-area, allowing
                    .prompt-area's z-index:2 to keep its buttons clickable. */}
                <div class="edge-swipe-zone edge-swipe-left" />
                <div class="edge-swipe-zone edge-swipe-right" />
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
