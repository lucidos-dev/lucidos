import { useEffect, useRef } from 'preact/hooks';

import {
  computeScrollIndicator,
  counterScaledRadiusPx,
  nextIndicatorVisibility,
} from '../components/chat/scrollIndicator';
import { getResizeMode } from '../components/chat/scrollState';
import { isRepaintNudging } from '../utils/iosRepaint';
import { isUserScrolling } from '../utils/scrollActivity';
import { viewportIsMobile } from '../utils/viewport';

/** How long the indicator lingers after the scroller STOPS MOVING, in ms.
 *
 *  Every real movement restarts it (see `nextIndicatorVisibility`), so this is a
 *  post-motion linger and not a budget the scroll has to finish inside. It was
 *  the latter once, which is what made the indicator vanish mid-fling. */
export const INDICATOR_HIDE_DELAY_MS = 1100;

/**
 * Drives the mobile transcript's own scroll indicator.
 *
 * See components/chat/scrollIndicator.ts for WHY we draw one instead of letting
 * WebKit draw its overlay indicator. This hook is the DOM half: it reads the
 * scroller's metrics on each scroll event and writes the result to the thumb.
 *
 * Two constraints shape it, both inherited from hooks/useHideOnScroll.ts, which
 * paid for them:
 *
 *  - **Nothing in the per-scroll path may write a layout property.** A layout
 *    write dirties layout, so the very next scroll event's `scrollTop` read
 *    forces a synchronous style+layout flush of the largest DOM in the app. The
 *    thumb is therefore moved AND sized by `transform` (`translateY` for the
 *    position, `scaleY` off the track-height base for the size), with `opacity`
 *    for the fade and `border-radius` to keep the caps round under that scale.
 *    None of the three affects layout, so the reads below stay cheap: they only
 *    force a flush if something already dirtied layout, and nothing here does.
 *    (`border-radius` is paint-only: it changes the painted shape of a box
 *    without moving or resizing anything.)
 *  - **Nothing writes a custom property on `documentElement`.** Custom
 *    properties inherit, so a scroll-frequency write on the root invalidates
 *    style for every node in the document. This hook writes inline styles on one
 *    element and reads no custom property at all.
 *
 * Visibility is decided by `nextIndicatorVisibility` (a pure function next to the
 * geometry, so both halves are unit-testable without a DOM). The rule it encodes:
 * a touch drag SUMMONS the indicator, and any real movement KEEPS it up. The two
 * signals are deliberately different, and that file documents why conflating
 * them made the indicator fade out mid-fling.
 */
export function useThreadScrollIndicator(opts: {
  scrollerRef: { current: HTMLElement | null };
  /** The track element, held in STATE by the caller (a callback ref), not in a
   *  ref. ThreadView renders a loading branch before the transcript branch, so
   *  the indicator mounts after the first effect pass; a plain ref is a stable
   *  object, so its `.current` filling in would never re-run this effect and the
   *  indicator would sit unwired for the rest of the thread's life. An element
   *  in state changes identity on mount, which is exactly the dependency the
   *  effect needs. */
  track: HTMLElement | null;
  /** The thumb element, in state for the same reason as `track`. */
  thumb: HTMLElement | null;
  /** Index of the first RENDERED exchange (see components/chat/threadWindow.ts). */
  renderFromIndex: number;
  /** Total exchanges in the thread, rendered or not. */
  totalExchanges: number;
}) {
  const { scrollerRef, track, thumb, renderFromIndex, totalExchanges } = opts;

  // The render window changes as the user scrolls up and ThreadView grows it.
  // Held in a ref rather than an effect dependency so a window growth does not
  // tear the scroll listener down and rebuild it mid-gesture.
  const windowRef = useRef({ renderFromIndex, totalExchanges });
  windowRef.current = { renderFromIndex, totalExchanges };

  // Re-run when the layout crosses the mobile breakpoint: the desktop transcript
  // sits below the header in normal flow, so its native scrollbar is already
  // aligned and this indicator is neither drawn nor driven there.
  const mobile = viewportIsMobile.value;

  useEffect(() => {
    if (!mobile) return;
    const scroller = scrollerRef.current;
    if (!scroller || !track || !thumb) return;

    // Measured, not assumed: the thumb's box is authored in rem, so it moves
    // with the UI scale, and `scaleY` needs the px value it is scaling FROM. All
    // are re-read by the ResizeObserver below rather than per frame.
    //
    // The height and the width deliberately use DIFFERENT APIs, and each is
    // wrong for the other's job: `getBoundingClientRect` reports the TRANSFORMED
    // box, so on the height it would return the already-scaled value and the
    // scale would compound frame over frame; `offsetWidth` rounds to a whole
    // pixel, which the radius maths below cannot absorb (see `measure`).
    let trackHeightPx = 0;
    let baseThumbHeightPx = 0;
    let thumbHalfWidthPx = 0;
    let shown = false;
    let hideTimer: ReturnType<typeof setTimeout> | null = null;

    function measure() {
      trackHeightPx = track!.clientHeight;
      baseThumbHeightPx = thumb!.offsetHeight;
      // Half the bar's width is the horizontal corner radius that makes each cap
      // a semicircle. Authored in rem, so it moves with the UI scale and has to
      // be read rather than assumed.
      //
      // getBoundingClientRect, NOT offsetWidth, which rounds to a whole pixel:
      // a rem width rarely lands on one, and rounding UP makes the two corner
      // radii on an edge sum to more than the width. CSS then scales EVERY
      // radius down by the overflow ratio, vertical ones included, which is
      // exactly the counter-scale this is feeding. The rect is the untransformed
      // width here because the thumb is only ever scaled on Y.
      thumbHalfWidthPx = thumb!.getBoundingClientRect().width / 2;
    }

    function paint() {
      const geo = computeScrollIndicator({
        scrollTop: scroller!.scrollTop,
        scrollHeight: scroller!.scrollHeight,
        clientHeight: scroller!.clientHeight,
        renderFromIndex: windowRef.current.renderFromIndex,
        totalExchanges: windowRef.current.totalExchanges,
        trackHeightPx,
      });
      if (!geo.visible || !(baseThumbHeightPx > 0)) {
        thumb!.style.opacity = '0';
        return;
      }
      const scaleY = geo.thumbHeightPx / baseThumbHeightPx;
      thumb!.style.transform = `translateY(${geo.thumbOffsetPx}px) scaleY(${scaleY})`;
      // Undo the scale's effect on the painted corner radius, so the caps stay
      // semicircular instead of stretching into points. Paint-only, so the
      // no-layout-writes rule above still holds.
      const radiusY = counterScaledRadiusPx(thumbHalfWidthPx, scaleY);
      thumb!.style.borderRadius = `${thumbHalfWidthPx}px / ${radiusY}px`;
      thumb!.style.opacity = shown ? '1' : '0';
    }

    function hide() {
      hideTimer = null;
      shown = false;
      thumb!.style.opacity = '0';
    }

    /** Re-read the two cached heights, then repaint against them.
     *
     *  Content growth (a streaming append, a window expansion) is deliberately
     *  NOT observed: it changes `scrollHeight`, but the indicator is only on
     *  screen while the user is dragging, and every scroll event during a drag
     *  re-reads `scrollHeight` anyway. Observing the transcript's content would
     *  buy a correction nobody can see, at the price of a callback per streamed
     *  token on the largest DOM in the app. */
    function refresh() {
      measure();
      paint();
    }

    function onScroll() {
      const next = nextIndicatorVisibility(shown, {
        userScrolling: isUserScrolling(),
        programmaticScroll: getResizeMode() === 'scroll',
        repaintNudge: isRepaintNudging(),
      });
      shown = next.shown;
      if (next.armHideTimer) {
        if (hideTimer) clearTimeout(hideTimer);
        hideTimer = setTimeout(hide, INDICATOR_HIDE_DELAY_MS);
      }
      paint();
    }

    refresh();
    scroller.addEventListener('scroll', onScroll, { passive: true });

    // The track's height moves with the pane (rotation, the keyboard resizing the
    // visual viewport, the header spacer collapsing), and the thumb's base height
    // moves with the UI scale.
    const resizeObserver = new ResizeObserver(refresh);
    resizeObserver.observe(track);

    return () => {
      scroller.removeEventListener('scroll', onScroll);
      resizeObserver.disconnect();
      if (hideTimer) clearTimeout(hideTimer);
      thumb.style.transform = '';
      thumb.style.opacity = '';
      thumb.style.borderRadius = '';
    };
  }, [mobile, scrollerRef, track, thumb]);
}
