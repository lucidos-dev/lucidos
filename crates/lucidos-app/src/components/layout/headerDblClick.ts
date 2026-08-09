import { setSplitRatio, toggleContentPaneRatio, toggleThreadPaneRatio } from './splitHelpers';

/** Which of the header bar's three pane segments a point falls in. The bar spans
 *  all three panes as one band, so a double-click on it has to be attributed
 *  before it can be answered. */
export type HeaderRegion = 'drawer' | 'thread' | 'content';

/**
 * The segment a header double-click landed in, from the same geometry the CSS
 * positions those segments with: `divider-x = co + ddo + sr * (100% - co - ddo)`
 * (see `.app-header::after` in styles/panels/shell.css).
 *
 * Pure, so the whole attribution is testable without a layout engine: the caller
 * does the DOM reads and passes the resolved pixels.
 *
 * By x rather than by hit-testing the region elements, because the segments are
 * not fully covered by them. `.threads-header` is only as tall as its 2.25rem
 * control row inside a 3rem bar, so a press in the few px above or below it
 * lands on the bar itself, and a `closest('.threads-header')` fence would let
 * exactly that sliver through to be attributed to a pane the point is not over.
 */
export function headerDblClickRegion(
  { x, headerLeft, headerWidth, drawerWidthPx, drawerDividerPx, ratio }: {
    x: number;
    headerLeft: number;
    headerWidth: number;
    /** `--content-offset`: the thread drawer's width, 0 when it is closed. */
    drawerWidthPx: number;
    /** `--divider-width` while the drawer is open, else 0. */
    drawerDividerPx: number;
    ratio: number;
  },
): HeaderRegion {
  if (x < headerLeft + drawerWidthPx) return 'drawer';
  const splitX = headerLeft + drawerWidthPx + drawerDividerPx
    + ratio * (headerWidth - drawerWidthPx - drawerDividerPx);
  return x < splitX ? 'thread' : 'content';
}

/**
 * Double-clicking the Conversation pane's header segment maximizes that pane
 * (toggles the Canvas pane collapsed); the Canvas pane's segment does the
 * reverse.
 *
 * The DRAWER's segment answers nothing. A header double-click acts on the
 * SPLIT, and the drawer sits on its own side of the drawer divider, so
 * maximizing a pane group from it reads as a different pane answering the
 * click. It used to do exactly that, because everything left of the split
 * divider counted as "the thread side", the drawer band included.
 */
export function resolveHeaderDblClick({ region, ratio }: { region: HeaderRegion; ratio: number }) {
  if (region === 'drawer') return;
  setSplitRatio(region === 'thread' ? toggleContentPaneRatio(ratio) : toggleThreadPaneRatio(ratio));
}
