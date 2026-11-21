import { useLayoutEffect, useState } from 'preact/hooks';
import type { RefObject } from 'preact';
import { viewportIsMobile } from '../utils/viewport';
import { getRemPx } from '../utils/dom';
import { contentWidthOf } from './useFitsInOneRow';
import { useFontMetricsResize } from '../components/chat/promptResize';

/** A desktop row narrower than this draws the compact faces. In rem, so the
 *  threshold follows the UI scale. */
export const COMPOSER_COMPACT_MAX_REM = 36;

/** Which face set the composer row draws: the compact faces (the intelligence
 *  badge alone, a round action button) or the desktop ones. The phone layout is
 *  always compact. A desktop row is compact when it is narrower than
 *  `COMPOSER_COMPACT_MAX_REM`. */
export function isComposerCompact(input: { phone: boolean; rowWidthPx: number; remPx: number }): boolean {
  return input.phone || input.rowWidthPx < COMPOSER_COMPACT_MAX_REM * input.remPx;
}

/** Whether the composer row draws the compact faces, measured on the row.
 *
 *  The pane sets the measured width, never the row's members. So a compact
 *  face cannot widen the row back over the threshold.
 *  A UI scale change moves the threshold but not the row, so the font metrics
 *  re-measure too. Reading `viewportIsMobile.value` subscribes the caller to
 *  the phone layout. */
export function useComposerCompact(rowRef: RefObject<HTMLElement>): boolean {
  const phone = viewportIsMobile.value;
  const [narrow, setNarrow] = useState(false);
  const [fontMetrics, setFontMetrics] = useState(0);
  useFontMetricsResize(() => setFontMetrics((n) => n + 1));
  useLayoutEffect(() => {
    const row = rowRef.current;
    if (!row) return;
    const measure = () => setNarrow(
      isComposerCompact({ phone: false, rowWidthPx: contentWidthOf(row), remPx: getRemPx() }),
    );
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(row);
    return () => ro.disconnect();
  }, [rowRef, fontMetrics]);
  return phone || narrow;
}
