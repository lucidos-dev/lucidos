import { setSplitRatio, toggleContentPaneRatio, toggleThreadPaneRatio } from './splitHelpers';
import { splitFitsBoth } from './drawnLayout';

export type HeaderRegion = 'drawer' | 'thread' | 'content';

export function headerDblClickRegion(
  { x, headerLeft, headerWidth, drawerWidthPx, drawerDividerPx, ratio }: {
    x: number;
    headerLeft: number;
    headerWidth: number;
    drawerWidthPx: number;
    drawerDividerPx: number;
    ratio: number;
  },
): HeaderRegion {
  if (x < headerLeft + drawerWidthPx) return 'drawer';
  const splitX = headerLeft + drawerWidthPx + drawerDividerPx
    + ratio * (headerWidth - drawerWidthPx - drawerDividerPx);
  return x < splitX ? 'thread' : 'content';
}

export function resolveHeaderDblClick({ region, ratio }: { region: HeaderRegion; ratio: number }) {
  if (region === 'drawer') return;
  setSplitRatio(region === 'thread' ? toggleContentPaneRatio(ratio, splitFitsBoth()) : toggleThreadPaneRatio(ratio));
}
