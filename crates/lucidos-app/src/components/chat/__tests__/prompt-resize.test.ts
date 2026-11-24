import { describe, it, expect, afterEach, vi } from 'vitest';
import {
  resizeTextarea,
  remeasureTextarea,
  isTextareaHeightAnimating,
  animateTextareaHeightFrom,
  easeEmptiedTextarea,
  remeasureTextareaForPlaceholder,
} from '../promptResize';
import { motionPreference } from '../../../utils/motion';

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
    getBoundingClientRect: () => ({ width: 300 }),
  };

  return el as unknown as HTMLTextAreaElement;
}

function collapseBox(el: HTMLTextAreaElement) {
  Object.assign(el, { getBoundingClientRect: () => ({ width: 0 }) });
}

function expandBox(el: HTMLTextAreaElement) {
  Object.assign(el, { getBoundingClientRect: () => ({ width: 300 }) });
}

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

  it('grows an empty box to fit a placeholder that wraps', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    withPlaceholder(el, 'Type custom answer here…', () => 56);
    resizeTextarea(el);
    expect(el.style.height).toBe('56px');
  });

  it('ignores the placeholder once the field carries a value', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: 'ok' });
    withPlaceholder(el, 'Type custom answer here…', () => 56);
    resizeTextarea(el);
    expect(el.style.height).toBe('36px');
  });

  it('does not notice a placeholder swap on its own (why remeasure exists)', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    let needed = 36;
    withPlaceholder(el, 'Post a follow up…', () => needed);
    resizeTextarea(el);
    expect(el.style.height).toBe('36px');

    needed = 56;
    expect(resizeTextarea(el)).toBe(false);
    expect(el.style.height).toBe('36px');

    expect(remeasureTextarea(el)).toBe(true);
    expect(el.style.height).toBe('56px');
  });

  it('remeasures back down when the question is answered', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    let needed = 56;
    withPlaceholder(el, 'Type custom answer here…', () => needed);
    resizeTextarea(el);
    expect(el.style.height).toBe('56px');

    needed = 36;
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

describe('a box with no width is never measured', () => {
  it('resizeTextarea leaves the height standing while the pane is collapsed', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 56, value: 'two lines' });
    resizeTextarea(el);
    expect(el.style.height).toBe('56px');

    collapseBox(el);
    Object.defineProperty(el, 'scrollHeight', { get: () => 900, configurable: true });
    el.value = 'two lines and more';

    expect(resizeTextarea(el)).toBe(false);
    expect(el.style.height).toBe('56px');
  });

  it('remeasureTextarea stands down too, so a placeholder swap cannot poison it', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    withPlaceholder(el, 'Post a follow up…', () => 36);
    resizeTextarea(el);
    expect(el.style.height).toBe('36px');

    collapseBox(el);
    Object.defineProperty(el, 'scrollHeight', { get: () => 900, configurable: true });

    expect(remeasureTextarea(el)).toBe(false);
    expect(el.style.height).toBe('36px');
  });

  it('measures again once the pane gives the width back', () => {
    const el = createMockTextarea({ minHeight: 36, maxHeight: 400, contentHeight: 36, value: '' });
    withPlaceholder(el, 'Type custom answer here…', () => 56);
    collapseBox(el);
    expect(remeasureTextarea(el)).toBe(false);

    expandBox(el);
    expect(remeasureTextarea(el)).toBe(true);
    expect(el.style.height).toBe('56px');
  });
});

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
    frames.shift()?.();
    frames.shift()?.();
    vi.advanceTimersByTime(400);
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });

  it('is false when there was nothing to animate (same height)', () => {
    const { el } = animatingTextarea();
    animateTextareaHeightFrom(el, '120px');
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });
});

describe('easeEmptiedTextarea', () => {
  const frames: Array<() => void> = [];
  const origRaf = globalThis.requestAnimationFrame;
  const origCancelRaf = globalThis.cancelAnimationFrame;

  afterEach(() => {
    frames.length = 0;
    globalThis.requestAnimationFrame = origRaf;
    globalThis.cancelAnimationFrame = origCancelRaf;
    motionPreference.value = 'system';
    vi.useRealTimers();
  });

  function emptiedBox() {
    globalThis.requestAnimationFrame = ((cb: FrameRequestCallback) => {
      frames.push(() => cb(0));
      return frames.length;
    }) as typeof globalThis.requestAnimationFrame;
    globalThis.cancelAnimationFrame = (() => {}) as typeof globalThis.cancelAnimationFrame;
    const content = { height: 120 };
    const el = createMockTextarea({ minHeight: 44, maxHeight: 400, contentHeight: 0, value: '' });
    Object.defineProperty(el, 'scrollHeight', {
      get: () => Math.max(content.height, Math.min(Math.max(parseInt(el.style.height) || 0, 44), 400)),
    });
    Object.assign(el, { addEventListener() {}, removeEventListener() {} });
    content.height = 120;
    resizeTextarea(el);
    expect(el.style.height).toBe('120px');
    content.height = 44;
    return { el, content };
  }

  it('parks the box at its old height and eases it to the empty one', () => {
    const { el } = emptiedBox();
    easeEmptiedTextarea(el);
    expect(el.style.height).toBe('120px');
    expect(isTextareaHeightAnimating(el)).toBe(true);
    frames.shift()?.();
    frames.shift()?.();
    expect(el.style.transition).toContain('height');
    expect(el.style.height).toBe('44px');
  });

  it('snaps under reduced motion', () => {
    motionPreference.value = 'reduce';
    const { el } = emptiedBox();
    easeEmptiedTextarea(el);
    expect(el.style.height).toBe('44px');
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });

  it('re-aims an ease in flight at the new placeholder height', () => {
    const { el, content } = emptiedBox();
    easeEmptiedTextarea(el);
    frames.shift()?.();
    frames.shift()?.();
    const computed = vi.spyOn(globalThis, 'getComputedStyle')
      .mockReturnValue({ height: '80px' } as CSSStyleDeclaration);
    content.height = 30;
    remeasureTextareaForPlaceholder(el);
    computed.mockRestore();
    expect(el.style.height).toBe('80px');
    expect(isTextareaHeightAnimating(el)).toBe(true);
    frames.shift()?.();
    frames.shift()?.();
    expect(el.style.height).toBe('44px');
  });

  it('re-aims an ease whose transition just started without keeping its height', () => {
    const { el, content } = emptiedBox();
    let live: { transitionProperty: string; cancel: () => void } | null = null;
    const startedHeight = 120;
    Object.defineProperty(el, 'scrollHeight', {
      get: () => Math.max(
        content.height,
        Math.min(Math.max(parseInt(el.style.height) || 0, 44), 400),
        live ? startedHeight : 0,
      ),
    });
    const opacity = { transitionProperty: 'opacity', cancel: vi.fn() };
    Object.assign(el, { getAnimations: () => (live ? [live, opacity] : [opacity]) });
    easeEmptiedTextarea(el);
    frames.shift()?.();
    frames.shift()?.();
    live = { transitionProperty: 'height', cancel: () => { live = null; } };
    const computed = vi.spyOn(globalThis, 'getComputedStyle')
      .mockReturnValue({ height: `${startedHeight}px` } as CSSStyleDeclaration);
    remeasureTextareaForPlaceholder(el);
    computed.mockRestore();
    expect(live).toBeNull();
    expect(opacity.cancel).not.toHaveBeenCalled();
    frames.shift()?.();
    frames.shift()?.();
    expect(el.style.height).toBe('44px');
  });

  it('just re-measures with no ease in flight', () => {
    const { el } = emptiedBox();
    remeasureTextareaForPlaceholder(el);
    expect(el.style.height).toBe('44px');
    expect(isTextareaHeightAnimating(el)).toBe(false);
  });

  it('gives way to a height the typing writes during the ease', () => {
    vi.useFakeTimers();
    const { el, content } = emptiedBox();
    easeEmptiedTextarea(el);
    frames.shift()?.();
    frames.shift()?.();
    el.value = 'a\nb\nc';
    content.height = 90;
    Object.defineProperty(el, 'clientHeight', { get: () => 44 });
    resizeTextarea(el);
    expect(isTextareaHeightAnimating(el)).toBe(false);
    vi.advanceTimersByTime(1_000);
    expect(el.style.height).toBe('90px');
  });
});
