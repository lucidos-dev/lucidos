import { useEffect } from 'preact/hooks';

import {
  computeScrollIndicator,
  counterScaledRadiusPx,
  nextIndicatorVisibility,
} from '../components/chat/scrollIndicator';
import { isNavigationScroll } from '../components/chat/scrollState';
import { isRepaintNudging } from '../utils/webkitRepaint';
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
 * See components/chat/scrollIndicator.ts for WHY mobile draws one. This hook is
 * the DOM half: it reads the scroller's metrics on each scroll event and writes
 * the result to the thumb. Desktop keeps the native scrollbar.
 *
 * Two constraints shape it, both inherited from hooks/useHideOnScroll.ts:
 *
 *  - **Nothing in the per-scroll path may write a layout property.** After a
 *    layout write, reading `scrollTop` in the next scroll event flushes style
 *    and layout for the largest DOM in the app. So the thumb is moved AND sized
 *    by `transform`, with `opacity` for the fade and `border-radius` to keep
 *    the caps round under the scale. None of the three affects layout.
 *  - **Nothing writes a custom property on `documentElement`.** A
 *    scroll-frequency write there invalidates style for every node.
 *
 * Visibility is `nextIndicatorVisibility`: a touch drag SUMMONS the indicator,
 * and any real movement KEEPS it up.
 */
export function useThreadScrollIndicator(opts: {
  scrollerRef: { current: HTMLElement | null };
  /** The track element, held in STATE by the caller (a callback ref), not in a
   *  ref. ThreadView renders a loading branch before the transcript branch, so
   *  the indicator mounts after the first effect pass; a plain ref filling in
   *  would never re-run this effect. */
  track: HTMLElement | null;
  /** The thumb element, in state for the same reason as `track`. */
  thumb: HTMLElement | null;
}) {
  const { scrollerRef, track, thumb } = opts;

  // Re-run when the layout crosses the mobile breakpoint.
  const mobile = viewportIsMobile.value;

  useEffect(() => {
    if (!mobile) return;
    const scroller = scrollerRef.current;
    if (!scroller || !track || !thumb) return;

    // Measured, not assumed: the thumb's box is authored in rem, so it moves
    // with the UI scale, and `scaleY` needs the px value it is scaling FROM.
    //
    // The height and the width use DIFFERENT APIs, each wrong for the other's
    // job. `getBoundingClientRect` reports the TRANSFORMED box, so on the height
    // the scale would compound frame over frame. `offsetWidth` rounds to a whole
    // pixel. A width rounded up makes the corner radii overflow, and CSS then
    // shrinks every radius, which defeats the counter-scale.
    let trackHeightPx = 0;
    let baseThumbHeightPx = 0;
    let thumbHalfWidthPx = 0;
    let shown = false;
    let hideTimer: ReturnType<typeof setTimeout> | null = null;

    function measure() {
      trackHeightPx = track!.clientHeight;
      baseThumbHeightPx = thumb!.offsetHeight;
      thumbHalfWidthPx = thumb!.getBoundingClientRect().width / 2;
    }

    function paint() {
      const geo = computeScrollIndicator({
        scrollTop: scroller!.scrollTop,
        scrollHeight: scroller!.scrollHeight,
        clientHeight: scroller!.clientHeight,
        trackHeightPx,
      });
      if (!geo.visible || !(baseThumbHeightPx > 0)) {
        thumb!.style.opacity = '0';
        return;
      }
      const scaleY = geo.thumbHeightPx / baseThumbHeightPx;
      thumb!.style.transform = `translateY(${geo.thumbOffsetPx}px) scaleY(${scaleY})`;
      const radiusY = counterScaledRadiusPx(thumbHalfWidthPx, scaleY);
      thumb!.style.borderRadius = `${thumbHalfWidthPx}px / ${radiusY}px`;
      thumb!.style.opacity = shown ? '1' : '0';
    }

    function hide() {
      hideTimer = null;
      shown = false;
      thumb!.style.opacity = '0';
    }

    /** Re-read the cached sizes, then repaint against them.
     *
     *  Content growth is NOT observed. The indicator is on screen only while the
     *  reader scrolls, and every scroll event re-reads `scrollHeight` anyway. An
     *  observer would cost a callback per streamed token for nothing visible. */
    function refresh() {
      measure();
      paint();
    }

    function onScroll() {
      const next = nextIndicatorVisibility(shown, {
        userScrolling: isUserScrolling(),
        programmaticScroll: isNavigationScroll(scroller),
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
