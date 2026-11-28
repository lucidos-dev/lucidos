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

    resolveHeaderDblClick({ region: 'content', ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(true);

    resolveHeaderDblClick({ region: 'content', ratio: 0 });
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('drawer stays closed through full collapse/expand cycle', () => {
    threadDrawerOpen.value = false;

    resolveHeaderDblClick({ region: 'content', ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(false);

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
  const bar = {
    headerLeft: 40,
    headerWidth: 1280,
    drawerWidthPx: 300,
    drawerDividerPx: 6,
    ratio: DEFAULT_SPLIT_RATIO,
  };
  const at = (x: number) => headerDblClickRegion({ ...bar, x });

  it('names the drawer for anything over the drawer', () => {
    expect(at(40)).toBe('drawer');
    expect(at(200)).toBe('drawer');
    expect(at(339)).toBe('drawer');
  });

  it('names the Conversation pane from the drawer divider to the split', () => {
    expect(at(340)).toBe('thread');
    expect(at(700)).toBe('thread');
  });

  it('names the Canvas pane past the split divider', () => {
    expect(at(760)).toBe('content');
    expect(at(1300)).toBe('content');
  });

  it('has no drawer segment at all while the drawer is closed', () => {
    const closed = { ...bar, drawerWidthPx: 0, drawerDividerPx: 0, x: 41 };
    expect(headerDblClickRegion(closed)).toBe('thread');
  });

  it('attributes by x, so the sliver of bar above and below the control row counts', () => {
    expect(at(200)).toBe('drawer');
  });
});
