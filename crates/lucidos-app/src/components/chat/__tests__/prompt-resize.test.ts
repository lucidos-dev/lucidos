import { describe, it, expect } from 'vitest';
import { resizeTextarea } from '../promptResize';

/**
 * Creates a mock textarea with configurable layout properties.
 * Simulates CSS min-height, max-height, and content-driven scrollHeight
 * so we can test the resize algorithm without a real browser.
 */
function createMockTextarea(config: {
  minHeight: number;
  maxHeight: number;
  contentHeight: number;
  value?: string;
}) {
  const style: Record<string, string> = { height: '', overflowY: '' };

  function renderedHeight(): number {
    const h = parseInt(style.height) || 0;
    return Math.min(Math.max(h, config.minHeight), config.maxHeight);
  }

  const el = {
    style,
    scrollTop: 0,
    value: config.value ?? 'x'.repeat(20),
    get offsetHeight() { return renderedHeight(); },
    get clientHeight() { return renderedHeight(); },
    get scrollHeight() { return Math.max(config.contentHeight, renderedHeight()); },
  };

  return el as unknown as HTMLTextAreaElement;
}

describe('resizeTextarea', () => {
  it('keeps overflow-y hidden when content fits within max-height', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 49 });
    resizeTextarea(el);
    expect(el.style.overflowY).toBe('hidden');
  });

  it('enables overflow-y auto when content exceeds max-height', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 500 });
    resizeTextarea(el);
    expect(el.style.overflowY).toBe('auto');
  });

  it('resets scrollTop to 0 when content fits', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 49 });
    el.scrollTop = 5;
    resizeTextarea(el);
    expect(el.scrollTop).toBe(0);
  });

  it('preserves scrollTop when content exceeds max-height', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 500 });
    el.scrollTop = 50;
    resizeTextarea(el);
    expect(el.scrollTop).toBe(50);
  });

  it('returns true when height changed', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 49 });
    const changed = resizeTextarea(el);
    expect(changed).toBe(true);
  });

  it('returns false when height stays the same', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 30, value: 'hello' });
    resizeTextarea(el);
    el.value = 'hello!';
    const changed = resizeTextarea(el);
    expect(changed).toBe(false);
  });

  it('sets height to scrollHeight value', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 70 });
    resizeTextarea(el);
    expect(el.style.height).toBe('70px');
  });

  it('clamps to min-height when content is shorter', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 30 });
    resizeTextarea(el);
    // scrollHeight at collapsed height = max(30, 44) = 44
    expect(el.style.height).toBe('44px');
  });

  it('skips collapse when text grows by one char (fast path)', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 68, value: 'line one two' });
    resizeTextarea(el);

    el.value = 'line one two ';
    const changed = resizeTextarea(el);

    expect(changed).toBe(false);
    expect(el.style.height).toBe('68px');
  });

  it('collapses to re-measure when text jumps (paste)', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 68, value: 'short' });
    resizeTextarea(el);

    // Simulate pasting a lot of text — content height increases
    el.value = 'short plus a whole bunch of pasted text that spans multiple lines';
    Object.defineProperty(el, 'scrollHeight', {
      get() {
        const h = parseInt(el.style.height) || 0;
        const rendered = Math.min(Math.max(h, 44), 400);
        return Math.max(200, rendered);
      },
      configurable: true,
    });
    Object.defineProperty(el, 'clientHeight', {
      get() {
        const h = parseInt(el.style.height) || 0;
        return Math.min(Math.max(h, 44), 400);
      },
      configurable: true,
    });

    const changed = resizeTextarea(el);
    expect(changed).toBe(true);
    expect(el.style.height).toBe('200px');
  });

  it('collapses to measure when text shrinks', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 90, value: 'a long text here' });
    resizeTextarea(el);
    expect(el.style.height).toBe('90px');

    el.value = 'short';
    Object.defineProperty(el, 'scrollHeight', {
      get() {
        const h = parseInt(el.style.height) || 0;
        const rendered = Math.min(Math.max(h, 44), 400);
        return Math.max(68, rendered);
      },
      configurable: true,
    });
    Object.defineProperty(el, 'clientHeight', {
      get() {
        const h = parseInt(el.style.height) || 0;
        return Math.min(Math.max(h, 44), 400);
      },
      configurable: true,
    });

    const changed = resizeTextarea(el);
    expect(changed).toBe(true);
    expect(el.style.height).toBe('68px');
  });

  it('grows without collapse when content overflows', () => {
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 68, value: 'ab' });
    resizeTextarea(el);

    el.value = 'abc';
    Object.defineProperty(el, 'scrollHeight', {
      get() {
        const h = parseInt(el.style.height) || 0;
        const rendered = Math.min(Math.max(h, 44), 400);
        return Math.max(90, rendered);
      },
      configurable: true,
    });
    Object.defineProperty(el, 'clientHeight', {
      get() {
        const h = parseInt(el.style.height) || 0;
        return Math.min(Math.max(h, 44), 400);
      },
      configurable: true,
    });

    const changed = resizeTextarea(el);
    expect(changed).toBe(true);
    expect(el.style.height).toBe('90px');
  });
});
