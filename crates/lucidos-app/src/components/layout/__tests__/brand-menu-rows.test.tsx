// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { BrandMenuRows } from '../BrandMenuRows';
import { searchEverywhereAnchor, searchEverywhereOpen, whatsNewTargetRelease } from '../../../store/store';

let host: HTMLDivElement | undefined;

function mount(): HTMLDivElement {
  host = document.createElement('div');
  document.body.append(host);
  return host;
}

beforeEach(() => {
  searchEverywhereAnchor.value = null;
  searchEverywhereOpen.value = false;
  whatsNewTargetRelease.value = null;
});

afterEach(() => {
  if (host) { render(null, host); host.remove(); host = undefined; }
});

describe('the Lucidos menu rows', () => {
  it('opens the search palette with no anchor, so a menu row never anchors it', async () => {
    const onClose = vi.fn();
    const el = mount();
    const stale = document.createElement('button');
    searchEverywhereAnchor.value = stale;
    await act(async () => {
      render(<BrandMenuRows onClose={onClose} actionsInRow={false} homeInRow={false} />, el);
    });
    const row = [...el.querySelectorAll('button')].find((b) => b.textContent?.includes('Search everywhere'));
    expect(row).toBeDefined();
    await act(async () => { row!.click(); });
    expect(onClose).toHaveBeenCalled();
    expect(searchEverywhereOpen.value).toBe(true);
    expect(searchEverywhereAnchor.value).toBeNull();
  });

  it('opens What is new through the menu, which clears a target an update left behind', async () => {
    const onClose = vi.fn();
    const el = mount();
    whatsNewTargetRelease.value = '9.9.9';
    await act(async () => {
      render(<BrandMenuRows onClose={onClose} actionsInRow={false} homeInRow={false} />, el);
    });
    const row = el.querySelector<HTMLButtonElement>('.brand-menu-version');
    expect(row).not.toBeNull();
    await act(async () => { row!.click(); });
    expect(onClose).toHaveBeenCalled();
    expect(whatsNewTargetRelease.value).toBeNull();
  });
});
