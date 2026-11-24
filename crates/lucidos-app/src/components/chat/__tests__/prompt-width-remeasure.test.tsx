// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import {
  useWidthRemeasure,
  animateTextareaHeightFrom,
  isTextareaHeightAnimating,
} from '../promptResize';

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

function shape(el: HTMLTextAreaElement, width: number, contentHeight: number) {
  Object.defineProperty(el, 'getBoundingClientRect', {
    value: () => ({ width }), configurable: true,
  });
  for (const prop of ['scrollHeight', 'offsetHeight', 'clientHeight']) {
    Object.defineProperty(el, prop, { get: () => contentHeight, configurable: true });
  }
}

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

    shape(el, 300, 90);
    deliverResize();
    expect(el.style.height).toBe('');
  });

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
