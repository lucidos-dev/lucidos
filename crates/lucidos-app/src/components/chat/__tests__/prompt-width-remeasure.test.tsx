// @vitest-environment jsdom
/**
 * `useWidthRemeasure` watches the box, not the mount.
 *
 * The trigger form unmounts its Intent textarea whenever the run type changes.
 * So the ref is null on the render that opens a script trigger, and holds an
 * element only later. A mount-only effect reads that null once and never looks
 * again, leaving the field with no observer for the life of the form.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import {
  useWidthRemeasure,
  animateTextareaHeightFrom,
  isTextareaHeightAnimating,
} from '../promptResize';

/** Every live observer's callback. `deliverResize` is the layout engine jsdom
 *  does not have: it fires them all, as a real resize would. */
let callbacks: Array<() => void> = [];

class FakeResizeObserver {
  constructor(private readonly cb: () => void) {}
  observe() { callbacks.push(this.cb); }
  unobserve() {}
  disconnect() { callbacks = callbacks.filter((c) => c !== this.cb); }
}

function deliverResize() {
  for (const cb of [...callbacks]) cb();
}

/** Give a jsdom element the layout it would have in a browser. */
function shape(el: HTMLTextAreaElement, width: number, contentHeight: number) {
  Object.defineProperty(el, 'getBoundingClientRect', {
    value: () => ({ width }), configurable: true,
  });
  for (const prop of ['scrollHeight', 'offsetHeight', 'clientHeight']) {
    Object.defineProperty(el, prop, { get: () => contentHeight, configurable: true });
  }
}

/** Preact defers `useEffect` to a frame, and falls back to a 100ms timer where
 *  there are no frames. jsdom is that case, so a microtask is far too short. */
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 120));
}

function Field({ present }: { present: boolean }) {
  const ref = useRef<HTMLTextAreaElement>(null);
  useWidthRemeasure(ref);
  return present ? <textarea ref={ref} /> : <div />;
}

describe('useWidthRemeasure', () => {
  let host: HTMLElement;

  beforeEach(() => {
    callbacks = [];
    globalThis.ResizeObserver = FakeResizeObserver as unknown as typeof ResizeObserver;
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('attaches to a box that only arrives on a later render', async () => {
    render(<Field present={false} />, host);
    await settle();
    expect(callbacks).toHaveLength(0);

    render(<Field present />, host);
    const el = host.querySelector('textarea')!;
    shape(el, 300, 50);
    await settle();
    expect(callbacks, 'the box that arrived late is observed').toHaveLength(1);

    shape(el, 150, 90);
    deliverResize();
    expect(el.style.height).toBe('90px');
  });

  it('ignores a resize that left the width alone', async () => {
    render(<Field present />, host);
    const el = host.querySelector('textarea')!;
    shape(el, 300, 50);
    await settle();

    // Our own height write re-enters the observer. Acting on it would loop.
    shape(el, 300, 90);
    deliverResize();
    expect(el.style.height).toBe('');
  });

  // A draft-switch ease is heading for a height measured at the old width, so
  // a width change invalidates it. Standing down would land the box on that
  // stale number and leave it there: the observer fires ON the width change,
  // and by then the width has finished changing.
  it('abandons an in-flight height ease rather than easing to a stale height', async () => {
    render(<Field present />, host);
    const el = host.querySelector('textarea')!;
    shape(el, 300, 50);
    await settle();

    el.style.height = '50px';
    animateTextareaHeightFrom(el, '120px');
    expect(isTextareaHeightAnimating(el)).toBe(true);

    shape(el, 150, 90);
    deliverResize();

    expect(isTextareaHeightAnimating(el), 'the stale ease was dropped').toBe(false);
    expect(el.style.height, 'and the box measured at the new width').toBe('90px');
  });

  it('stops observing a box that leaves', async () => {
    render(<Field present />, host);
    shape(host.querySelector('textarea')!, 300, 50);
    await settle();
    expect(callbacks).toHaveLength(1);

    render(<Field present={false} />, host);
    await settle();
    expect(callbacks).toHaveLength(0);
  });

  it('stops observing when the component unmounts', async () => {
    render(<Field present />, host);
    shape(host.querySelector('textarea')!, 300, 50);
    await settle();
    expect(callbacks).toHaveLength(1);

    render(null, host);
    await settle();
    expect(callbacks).toHaveLength(0);
  });
});
