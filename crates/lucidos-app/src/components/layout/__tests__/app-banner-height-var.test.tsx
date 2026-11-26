// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { act } from 'preact/test-utils';

const rem = vi.hoisted(() => ({ px: 16 }));
vi.mock('../../../utils/dom', async () => ({
  ...await vi.importActual('../../../utils/dom'),
  getRemPx: () => rem.px,
}));

import { useBannerHeightVar, bannerBelongsToLayout, type BannerLayout } from '../appBanner';

const VAR = '--test-banner-height';
let heightPx = 32;
let onResize: (() => void) | null = null;

class FakeResizeObserver {
  constructor(cb: () => void) { onResize = cb; }
  observe() {}
  disconnect() {}
}

function Probe({ layout, active }: { layout: BannerLayout; active: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useBannerHeightVar(ref, { layout, cssVar: VAR, active });
  return <div ref={ref} />;
}

function mountProbe(layout: BannerLayout, active = true): HTMLElement {
  const host = document.createElement('div');
  document.body.appendChild(host);
  act(() => render(<Probe layout={layout} active={active} />, host));
  return host;
}

const published = () => document.documentElement.style.getPropertyValue(VAR);

describe('a banner publishes its measured height in rem', () => {
  afterEach(() => {
    document.body.innerHTML = '';
    document.documentElement.style.removeProperty(VAR);
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
    rem.px = 16;
    heightPx = 32;
    onResize = null;
  });

  it('reads the root font size at each measure, so a UI scale change re-reserves the space', () => {
    vi.stubGlobal('ResizeObserver', FakeResizeObserver);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockImplementation(() => ({ height: heightPx }) as DOMRect);
    mountProbe('desktop');
    expect(published()).toBe('2rem');
    rem.px = 20;
    act(() => onResize!());
    expect(published()).toBe('1.6rem');
  });

  it('publishes nothing on the mobile layout, which lives in the header', () => {
    vi.stubGlobal('ResizeObserver', FakeResizeObserver);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockImplementation(() => ({ height: heightPx }) as DOMRect);
    mountProbe('mobile');
    expect(published()).toBe('');
  });

  it('clears its own property when it unmounts, so no reservation outlives the bar', () => {
    vi.stubGlobal('ResizeObserver', FakeResizeObserver);
    vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect')
      .mockImplementation(() => ({ height: heightPx }) as DOMRect);
    const host = mountProbe('desktop');
    expect(published()).toBe('2rem');
    act(() => render(null, host));
    expect(published()).toBe('');
  });
});

describe('bannerBelongsToLayout', () => {
  it('admits a banner only to the layout its viewport shows', () => {
    expect(bannerBelongsToLayout('desktop', false)).toBe(true);
    expect(bannerBelongsToLayout('mobile', true)).toBe(true);
    expect(bannerBelongsToLayout('mobile', false)).toBe(false);
    expect(bannerBelongsToLayout('desktop', true)).toBe(false);
  });
});
