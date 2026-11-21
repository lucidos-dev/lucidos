// @vitest-environment jsdom
/**
 * An anchored panel follows an anchor that moved on a transition.
 *
 * The phone's sticky title row glides back in on `translate` when the bars
 * reveal, with no scroll or resize. A widget dropped open from a transcript row
 * measures that title row mid-glide, so the hook re-measures when the anchor's
 * transition ends (ADR 0405).
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { act } from 'preact/test-utils';
import { useAnchoredPosition } from '../useAnchoredPopover';

let anchorTop = 0;
const anchor = document.createElement('div');
anchor.getBoundingClientRect = () => {
  const rect = { x: 0, y: anchorTop, left: 0, right: 300, top: anchorTop, bottom: anchorTop + 40, width: 300, height: 40 };
  return { ...rect, toJSON: () => rect } as DOMRect;
};

function Panel() {
  const ref = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(anchor, ref);
  return <div ref={ref} data-top={pos ? String(pos.top) : ''} />;
}

const panelTop = (host: HTMLElement) => Number(host.querySelector('div')?.getAttribute('data-top'));

describe('useAnchoredPosition', () => {
  let host: HTMLElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
    document.body.appendChild(anchor);
    Object.defineProperty(window, 'innerHeight', { configurable: true, value: 800 });
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    anchor.remove();
  });

  it('re-measures when the anchor finishes a transition', async () => {
    anchorTop = -40;
    await act(async () => { render(<Panel />, host); });
    const midGlide = panelTop(host);

    anchorTop = 60;
    await act(async () => {
      anchor.dispatchEvent(new Event('transitionend'));
      await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    });
    expect(panelTop(host)).toBeGreaterThan(midGlide);
  });
});
