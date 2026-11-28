import { useLayoutEffect, useState } from 'preact/hooks';
import type { RefObject } from 'preact';
import { viewportIsMobile } from '../utils/viewport';
import { getRemPx } from '../utils/dom';
import { contentWidthOf } from './useFitsInOneRow';
import { useFontMetricsResize } from '../components/chat/promptResize';

export const COMPOSER_COMPACT_MAX_REM = 36;

export function isComposerCompact(input: { phone: boolean; rowWidthPx: number; remPx: number }): boolean {
  return input.phone || input.rowWidthPx < COMPOSER_COMPACT_MAX_REM * input.remPx;
}

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
