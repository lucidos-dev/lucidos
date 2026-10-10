import { describe, it, expect, vi, afterEach } from 'vitest';
import { isKnownAppFrame } from './appFrame';

// The test env has no real DOM, so stub the iframe lookup. isKnownAppFrame's
// own logic — null guard + "is `source` the contentWindow of some app frame" —
// is what's under test, not the browser's querySelectorAll.
function withFrames(...frames: Array<{ contentWindow: unknown }>) {
  return vi.spyOn(document, 'querySelectorAll').mockReturnValue(frames as unknown as NodeListOf<Element>);
}

describe('isKnownAppFrame', () => {
  afterEach(() => vi.restoreAllMocks());

  it('returns false for a null source', () => {
    withFrames();
    expect(isKnownAppFrame(null)).toBe(false);
  });

  it('returns true when source is the content window of a mounted app iframe', () => {
    const win = {} as Window;
    withFrames({ contentWindow: win });
    expect(isKnownAppFrame(win)).toBe(true);
  });

  it('returns false when source matches no app iframe (top-level / unrelated window)', () => {
    withFrames({ contentWindow: {} });
    expect(isKnownAppFrame({} as Window)).toBe(false);
  });

  it('returns false when there are no app iframes mounted', () => {
    withFrames();
    expect(isKnownAppFrame({} as Window)).toBe(false);
  });
});
