import { describe, it, expect, beforeEach } from 'vitest';
import {
  MAX_SAVED_SCROLLS,
  SAVED_SCROLLS_KEY,
  contentScrollKey,
  pruneSavedScrolls,
  resetContentScroll,
  threadScrollKey,
  writeSavedScroll,
} from './savedScroll';

describe('saved scroll positions stay bounded', () => {
  beforeEach(() => localStorage.clear());

  it('forgets the least recently saved position past the cap', () => {
    for (let n = 0; n <= MAX_SAVED_SCROLLS; n++) writeSavedScroll(threadScrollKey(`t${n}`), String(n));
    writeSavedScroll(threadScrollKey('t1'), 'again');
    writeSavedScroll(threadScrollKey('t-new'), 'new');

    expect(localStorage.getItem(threadScrollKey('t0'))).toBeNull();
    expect(localStorage.getItem(threadScrollKey('t2'))).toBeNull();
    expect(localStorage.getItem(threadScrollKey('t1'))).toBe('again');
    expect(localStorage.getItem(threadScrollKey('t-new'))).toBe('new');
    expect(JSON.parse(localStorage.getItem(SAVED_SCROLLS_KEY)!)).toHaveLength(MAX_SAVED_SCROLLS);
  });

  it('sweeps positions the index does not hold', () => {
    writeSavedScroll(threadScrollKey('kept'), '1');
    localStorage.setItem(threadScrollKey('orphan'), '2');
    localStorage.setItem('lucidos-other', '3');

    pruneSavedScrolls();

    expect(localStorage.getItem(threadScrollKey('kept'))).toBe('1');
    expect(localStorage.getItem(threadScrollKey('orphan'))).toBeNull();
    expect(localStorage.getItem('lucidos-other')).toBe('3');
  });

  it('adopts existing positions up to the cap when no index exists yet', () => {
    for (let n = 0; n < MAX_SAVED_SCROLLS + 5; n++) localStorage.setItem(threadScrollKey(`t${n}`), String(n));

    pruneSavedScrolls();

    const kept = JSON.parse(localStorage.getItem(SAVED_SCROLLS_KEY)!) as string[];
    expect(kept).toHaveLength(MAX_SAVED_SCROLLS);
    const stored = Array.from({ length: localStorage.length }, (_, index) => localStorage.key(index)!)
      .filter((key) => key.startsWith('lucidos-scroll-'));
    expect(stored.sort()).toEqual([...kept].sort());
  });
});

describe('contentScrollKey', () => {
  it('matches the key shape ContentPane writes', () => {
    expect(contentScrollKey('triggers')).toBe('lucidos-scroll-content-triggers');
  });
});

describe('resetContentScroll', () => {
  beforeEach(() => localStorage.clear());

  it('removes the saved offset for the view', () => {
    localStorage.setItem('lucidos-scroll-content-triggers', '500');
    resetContentScroll('triggers');
    expect(localStorage.getItem('lucidos-scroll-content-triggers')).toBeNull();
  });

  it('is a no-op when nothing is saved', () => {
    expect(() => resetContentScroll('triggers')).not.toThrow();
  });

  it('does not touch other views', () => {
    localStorage.setItem('lucidos-scroll-content-triggers', '500');
    localStorage.setItem('lucidos-scroll-content-apps', '200');
    resetContentScroll('triggers');
    expect(localStorage.getItem('lucidos-scroll-content-apps')).toBe('200');
  });
});
