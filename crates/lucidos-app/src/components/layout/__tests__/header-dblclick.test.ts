import { describe, it, expect, beforeEach } from 'vitest';
import { splitRatio, threadDrawerOpen } from '../../../store/store';
import { DEFAULT_SPLIT_RATIO } from '../splitHelpers';
import { headerDblClickRegion, resolveHeaderDblClick } from '../headerDblClick';

describe('Header double-click: thread drawer state preservation', () => {
  beforeEach(() => {
    splitRatio.value = DEFAULT_SPLIT_RATIO;
    threadDrawerOpen.value = false;
  });

  it('content-side collapse preserves drawer open state', () => {
    threadDrawerOpen.value = true;

    resolveHeaderDblClick({ region: 'content', ratio: DEFAULT_SPLIT_RATIO });

    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('content-side expand preserves drawer open state', () => {
    threadDrawerOpen.value = true;
    splitRatio.value = 0;

    resolveHeaderDblClick({ region: 'content', ratio: 0 });

    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('drawer stays open through full collapse/expand cycle', () => {
    threadDrawerOpen.value = true;

    // Collapse
    resolveHeaderDblClick({ region: 'content', ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(true);

    // Expand: the drawer must reappear
    resolveHeaderDblClick({ region: 'content', ratio: 0 });
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('drawer stays closed through full collapse/expand cycle', () => {
    threadDrawerOpen.value = false;

    // Collapse
    resolveHeaderDblClick({ region: 'content', ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(false);

    // Expand: the drawer must stay closed
    resolveHeaderDblClick({ region: 'content', ratio: 0 });
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('thread-side collapse preserves drawer state', () => {
    threadDrawerOpen.value = true;

    resolveHeaderDblClick({ region: 'thread', ratio: DEFAULT_SPLIT_RATIO });

    expect(splitRatio.value).toBe(1);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('thread-side expand preserves drawer state', () => {
    threadDrawerOpen.value = true;
    splitRatio.value = 1;

    resolveHeaderDblClick({ region: 'thread', ratio: 1 });

    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });
});

describe('the thread drawer\'s own header segment answers no double-click', () => {
  beforeEach(() => {
    splitRatio.value = DEFAULT_SPLIT_RATIO;
    threadDrawerOpen.value = true;
  });

  it('moves nothing, where the other two segments move the split', () => {
    resolveHeaderDblClick({ region: 'drawer', ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('moves nothing from a collapsed split either, so it cannot un-collapse one', () => {
    splitRatio.value = 1;
    resolveHeaderDblClick({ region: 'drawer', ratio: 1 });
    expect(splitRatio.value).toBe(1);
  });
});

describe('headerDblClickRegion attributes a point to a pane segment', () => {
  // A 1280px bar at the default 0.4 ratio, with a 300px drawer open behind a
  // 6px divider: the drawer is [0, 300), the Conversation pane runs to the
  // split divider at 300 + 6 + 0.4 * 974 = 695.6, and the Canvas pane has the
  // rest. `headerLeft` is non-zero on purpose, so an implementation that forgot
  // to subtract it cannot pass.
  const bar = {
    headerLeft: 40,
    headerWidth: 1280,
    drawerWidthPx: 300,
    drawerDividerPx: 6,
    ratio: DEFAULT_SPLIT_RATIO,
  };
  const at = (x: number) => headerDblClickRegion({ ...bar, x });

  it('names the drawer for anything over the drawer', () => {
    expect(at(40)).toBe('drawer');       // the bar's own leading edge
    expect(at(200)).toBe('drawer');
    expect(at(339)).toBe('drawer');      // one px inside the drawer's trailing edge
  });

  it('names the Conversation pane from the drawer divider to the split', () => {
    expect(at(340)).toBe('thread');      // the drawer divider seam, which is not the drawer
    expect(at(700)).toBe('thread');
  });

  it('names the Canvas pane past the split divider', () => {
    expect(at(760)).toBe('content');     // 40 + 695.6, rounded clear
    expect(at(1300)).toBe('content');
  });

  it('has no drawer segment at all while the drawer is closed', () => {
    const closed = { ...bar, drawerWidthPx: 0, drawerDividerPx: 0, x: 41 };
    expect(headerDblClickRegion(closed)).toBe('thread');
  });

  it('attributes by x, so the sliver of bar above and below the control row counts', () => {
    // The reason this is geometry and not a `closest('.threads-header')` fence:
    // that row is 2.25rem inside a 3rem bar, so a press a few px from the bar's
    // edge lands on the bar itself. y never enters the arithmetic, which is
    // exactly what makes the whole segment inert rather than just the row.
    expect(at(200)).toBe('drawer');
  });
});
