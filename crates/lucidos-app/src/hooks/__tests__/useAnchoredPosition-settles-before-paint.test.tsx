// @vitest-environment jsdom
/**
 * An anchored panel opens where it will stay.
 *
 * The position is an input to the panel's own size: its `maxWidth` becomes the
 * caller's width cap, and a narrower panel wraps onto more lines. So an upward
 * panel must be placed by its height under the cap, never by its uncapped one.
 *
 * jsdom has no ResizeObserver and no layout, so nothing here re-measures except
 * the hook's own settling pass. The panel reports a taller height once the cap
 * is on it, which is what the wrap does in a browser.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { act } from 'preact/test-utils';
import { useAnchoredPosition, type AnchorBox } from '../useAnchoredPopover';

const UNCAPPED_HEIGHT = 50;
const CAPPED_HEIGHT = 80;

/** Bottom-docked, like the composer row the waiting indicator sits in. */
const ANCHOR: AnchorBox = {
  getBoundingClientRect: () => {
    const rect = { x: 20, y: 700, left: 20, right: 52, top: 700, bottom: 732, width: 32, height: 32 };
    return { ...rect, toJSON: () => rect };
  },
};

function Panel() {
  const ref = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(ANCHOR, ref);
  return (
    <div
      ref={ref}
      data-top={pos ? String(pos.top) : ''}
      style={pos ? { '--anchored-popover-fit': `${pos.maxWidth}px` } : { visibility: 'hidden' }}
    />
  );
}

describe('useAnchoredPosition', () => {
  let host: HTMLElement;
  let heightDescriptor: PropertyDescriptor | undefined;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    heightDescriptor = Object.getOwnPropertyDescriptor(HTMLElement.prototype, 'offsetHeight');
    Object.defineProperty(HTMLElement.prototype, 'offsetHeight', {
      configurable: true,
      get(this: HTMLElement) {
        return this.style.getPropertyValue('--anchored-popover-fit') ? CAPPED_HEIGHT : UNCAPPED_HEIGHT;
      },
    });
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 760 });
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    if (heightDescriptor) Object.defineProperty(HTMLElement.prototype, 'offsetHeight', heightDescriptor);
  });

  it('places an upward panel by the height it has under its own cap', async () => {
    await act(async () => {
      render(<Panel />, host);
    });
    const top = Number(host.querySelector('div')?.getAttribute('data-top'));
    // The anchor's top, less the gap, less the CAPPED height.
    expect(top).toBe(700 - 4 - CAPPED_HEIGHT);
  });
});
