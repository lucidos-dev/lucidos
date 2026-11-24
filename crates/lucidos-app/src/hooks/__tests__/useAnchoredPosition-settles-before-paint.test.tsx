// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { act } from 'preact/test-utils';
import { useAnchoredPosition, type AnchorBox } from '../useAnchoredPopover';

const UNCAPPED_HEIGHT = 50;
const CAPPED_HEIGHT = 80;

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
    expect(top).toBe(700 - 4 - CAPPED_HEIGHT);
  });
});
