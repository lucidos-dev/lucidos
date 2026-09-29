// @vitest-environment jsdom
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { StoreTabSkeleton } from '../StoreTab';

let host: HTMLDivElement | null = null;

function show(): HTMLDivElement {
  host = document.createElement('div');
  render(<StoreTabSkeleton />, host);
  return host;
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
});

describe('StoreTabSkeleton: mirrors the loaded layout so the list does not jump', () => {
  it('reserves a full category-pills bar above the list skeleton', () => {
    // Sized to a FULLY populated catalog: `All` plus ~9 categories, so the
    // skeleton's bar fills the way the real bar's does, scrolling or wrapped.
    const pills = show().querySelectorAll('.app-store-filter-pills .pill-bar-btn');
    expect(pills.length).toBeGreaterThanOrEqual(10);
  });

  it('draws each pill as the real pill box around a shimmering label', () => {
    const pill = show().querySelector('.app-store-filter-pills .pill-bar-btn');
    expect(pill?.tagName).toBe('SPAN');
    expect(pill?.querySelector('.sk-bar')).not.toBeNull();
  });

  it('draws plugin rows below the pills', () => {
    expect(show().querySelector('.app-store-plugins .sk-bar')).not.toBeNull();
  });
});
