import { describe, it, expect, beforeEach } from 'vitest';
import { splitRatio, threadDrawerOpen } from '../../../store/store';
import { DEFAULT_SPLIT_RATIO } from '../splitHelpers';
import { resolveHeaderDblClick } from '../headerDblClick';

describe('Header double-click: thread drawer state preservation', () => {
  beforeEach(() => {
    splitRatio.value = DEFAULT_SPLIT_RATIO;
    threadDrawerOpen.value = false;
  });

  it('content-side collapse preserves drawer open state', () => {
    threadDrawerOpen.value = true;

    resolveHeaderDblClick({ clickedThreadSide: false, ratio: DEFAULT_SPLIT_RATIO });

    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('content-side expand preserves drawer open state', () => {
    threadDrawerOpen.value = true;
    splitRatio.value = 0;

    resolveHeaderDblClick({ clickedThreadSide: false, ratio: 0 });

    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('drawer stays open through full collapse/expand cycle', () => {
    threadDrawerOpen.value = true;

    // Collapse
    resolveHeaderDblClick({ clickedThreadSide: false, ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(true);

    // Expand — drawer must reappear
    resolveHeaderDblClick({ clickedThreadSide: false, ratio: 0 });
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('drawer stays closed through full collapse/expand cycle', () => {
    threadDrawerOpen.value = false;

    // Collapse
    resolveHeaderDblClick({ clickedThreadSide: false, ratio: DEFAULT_SPLIT_RATIO });
    expect(splitRatio.value).toBe(0);
    expect(threadDrawerOpen.value).toBe(false);

    // Expand — drawer must stay closed
    resolveHeaderDblClick({ clickedThreadSide: false, ratio: 0 });
    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(false);
  });

  it('thread-side collapse preserves drawer state', () => {
    threadDrawerOpen.value = true;

    resolveHeaderDblClick({ clickedThreadSide: true, ratio: DEFAULT_SPLIT_RATIO });

    expect(splitRatio.value).toBe(1);
    expect(threadDrawerOpen.value).toBe(true);
  });

  it('thread-side expand preserves drawer state', () => {
    threadDrawerOpen.value = true;
    splitRatio.value = 1;

    resolveHeaderDblClick({ clickedThreadSide: true, ratio: 1 });

    expect(splitRatio.value).toBe(DEFAULT_SPLIT_RATIO);
    expect(threadDrawerOpen.value).toBe(true);
  });
});
