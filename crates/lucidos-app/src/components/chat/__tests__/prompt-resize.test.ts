import { describe, it, expect, afterEach, vi } from 'vitest';
import {
  resizeTextarea,
  remeasureTextarea,
  isTextareaHeightAnimating,
  animateTextareaHeightFrom,
} from '../promptResize';

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

/** Give a mock textarea a placeholder that needs `needed` px to render whole.
 *  placeholderHeight() measures a CLONE appended to the parent, so the mock
 *  supplies both: the clone reports the height, the parent absorbs the append. */
function withPlaceholder(el: HTMLTextAreaElement, text: string, needed: () => number) {
  const probe = {
    style: {} as Record<string, string>,
    value: '',
    removeAttribute() {},
    remove() {},
    get scrollHeight() { return needed(); },
  };
  Object.assign(el, {
    placeholder: text,
    cloneNode: () => probe,
    parentElement: { appendChild() {} },
    getBoundingClientRect: () => ({ width: 300 }),
  });
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

  // A textarea sizes to its VALUE, so an empty box measures one line however
  // long the placeholder is, and the composer's `overflow-y: hidden` clips the
  // rest. The answering placeholder is a whole sentence and wraps at phone
  // widths, in a narrowed thread pane, and at large UI scales.
  it('grows an empty box to fit a placeholder that wraps', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    withPlaceholder(el, 'Type your answer, or Cancel to ask something else.', () => 56);
    resizeTextarea(el);
    expect(el.style.height).toBe('56px');
  });

  it('ignores the placeholder once the field carries a value', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: 'ok' });
    withPlaceholder(el, 'Type your answer, or Cancel to ask something else.', () => 56);
    resizeTextarea(el);
    expect(el.style.height).toBe('36px');
  });

  // The placeholder swap is invisible to resizeTextarea: the value is unchanged,
  // so the fast path returns before anything is re-measured. That is what
  // remeasureTextarea is for, on arrival AND on release.
  it('does not notice a placeholder swap on its own (why remeasure exists)', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    let needed = 36;
    withPlaceholder(el, 'Post a follow up…', () => needed);
    resizeTextarea(el);
    expect(el.style.height).toBe('36px');

    needed = 56; // a question arrives: the placeholder is now a whole sentence
    expect(resizeTextarea(el)).toBe(false);
    expect(el.style.height).toBe('36px');

    expect(remeasureTextarea(el)).toBe(true);
    expect(el.style.height).toBe('56px');
  });

  it('remeasures back down when the question is answered', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    let needed = 56;
    withPlaceholder(el, 'Type your answer, or Cancel to ask something else.', () => needed);
    resizeTextarea(el);
    expect(el.style.height).toBe('56px');

    needed = 36; // answered: the short follow-up placeholder is back
    expect(remeasureTextarea(el)).toBe(true);
    expect(el.style.height).toBe('36px');
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

// The compose FLIP eases the box between two drafts' heights by inverting:
// park it at the height it came from, then transition to the target it already
// rests at. Anything that writes a measured height inside that window lands the
// box ON the target before the transition starts, and the ease then plays over
// zero distance (transition engaged, nothing moved). The flag is how callers
// know to stand down; e2e/prompt-flip-height.spec.ts is what catches the real
// thing when they do not.
describe('isTextareaHeightAnimating', () => {
  const frames: Array<() => void> = [];
  const origRaf = globalThis.requestAnimationFrame;
  const origCancelRaf = globalThis.cancelAnimationFrame;

  afterEach(() => {
    frames.length = 0;
    globalThis.requestAnimationFrame = origRaf;
    globalThis.cancelAnimationFrame = origCancelRaf;
    vi.useRealTimers();
  });

  function animatingTextarea() {
    globalThis.requestAnimationFrame = ((cb: FrameRequestCallback) => {
      frames.push(() => cb(0));
      return frames.length;
    }) as typeof globalThis.requestAnimationFrame;
    globalThis.cancelAnimationFrame = (() => {}) as typeof globalThis.cancelAnimationFrame;
    const listeners: Record<string, (e?: unknown) => void> = {};
    const el = {
      style: { height: '120px', transition: '' },
      offsetHeight: 120,
      addEventListener: (type: string, fn: (e?: unknown) => void) => { listeners[type] = fn; },
      removeEventListener: (type: string) => { delete listeners[type]; },
    } as unknown as HTMLTextAreaElement;
    return { el, listeners };
  }

  it('is false for a textarea nobody is animating', () => {
    const { el } = animatingTextarea();
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });

  it('is true from the moment the animation is armed until it finishes', () => {
    vi.useFakeTimers();
    const { el, listeners } = animatingTextarea();
    animateTextareaHeightFrom(el, '44px');
    // Armed synchronously, before the two frames that start the transition:
    // the placeholder effect runs in the same commit and would otherwise slip in
    // right here.
    expect(isTextareaHeightAnimating(el)).toBe(true);

    frames.shift()?.();
    frames.shift()?.();
    expect(isTextareaHeightAnimating(el)).toBe(true);

    listeners.transitionend?.();
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });

  it('clears when the safety-net timer finishes an animation whose transitionend never fires', () => {
    vi.useFakeTimers();
    const { el } = animatingTextarea();
    animateTextareaHeightFrom(el, '44px');
    vi.advanceTimersByTime(400);
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });

  it('is false when there was nothing to animate (same height)', () => {
    const { el } = animatingTextarea();
    animateTextareaHeightFrom(el, '120px');
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });
});
