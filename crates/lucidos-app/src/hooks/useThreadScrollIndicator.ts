import { useEffect } from 'preact/hooks';

import {
  computeScrollIndicator,
  counterScaledRadiusPx,
  nextIndicatorVisibility,
} from '../components/chat/scrollIndicator';
import { onClassifiedScroll } from '../components/chat/classifiedScroll';
import type { ScrollClass } from '../components/chat/scrollState';
import { isRepaintNudging } from '../utils/webkitRepaint';
import { isUserScrolling } from '../utils/scrollActivity';
import { viewportIsMobile } from '../utils/viewport';

export const INDICATOR_HIDE_DELAY_MS = 1100;

export function useThreadScrollIndicator(opts: {
  scrollerRef: { current: HTMLElement | null };
  track: HTMLElement | null;
  thumb: HTMLElement | null;
}) {
  const { scrollerRef, track, thumb } = opts;

  const mobile = viewportIsMobile.value;

  useEffect(() => {
    if (!mobile) return;
    const scroller = scrollerRef.current;
    if (!scroller || !track || !thumb) return;

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

    function refresh() {
      measure();
      paint();
    }

    function onScroll(_told: ScrollClass, event: ScrollClass) {
      const next = nextIndicatorVisibility(shown, {
        userScrolling: isUserScrolling(),
        programmaticScroll: event.navigation,
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
    const stopScroll = onClassifiedScroll(scroller, onScroll);

    const resizeObserver = new ResizeObserver(refresh);
    resizeObserver.observe(track);

    return () => {
      stopScroll();
      resizeObserver.disconnect();
      if (hideTimer) clearTimeout(hideTimer);
      thumb.style.transform = '';
      thumb.style.opacity = '';
      thumb.style.borderRadius = '';
    };
  }, [mobile, scrollerRef, track, thumb]);
}
