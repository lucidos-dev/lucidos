// @vitest-environment jsdom
/**
 * The Apps and Plugins search bar rolls through a `<Disclosure>`, and its
 * field takes focus on every open. That includes a reopen during the exit
 * roll, when the row is still mounted and no mount effect runs again.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { AppSearchBar } from '../AppSearchBar';
import { useKeyboardShortcuts } from '../../../hooks/useKeyboardShortcuts';
import { activeMenuItem, panelOverlay, appSearchQuery, focusedPane } from '../../../store/store';
import { appSearchOpen } from '../../../store/store';
import { openAppSearch } from '../../../store/actions/apps';

let host: HTMLElement | null = null;
const jsdomAnimate = HTMLElement.prototype.animate;

function Harness() {
  useKeyboardShortcuts();
  return <AppSearchBar placeholder="Search apps…" dataRole="apps-search-input" />;
}

function mount() {
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<Harness />, host!); });
}

const field = () => host!.querySelector<HTMLInputElement>('[data-role="apps-search-input"]');

async function setOpen(open: boolean) {
  await act(async () => {
    appSearchOpen.value = open;
    await Promise.resolve();
  });
}

beforeEach(() => { appSearchOpen.value = false; });

afterEach(() => {
  vi.restoreAllMocks();
  HTMLElement.prototype.animate = jsdomAnimate;
  if (host) { render(null, host); host.remove(); host = null; }
  appSearchOpen.value = false;
});

describe('the app search bar', () => {
  it('draws nothing while shut, and focuses the field as it opens', async () => {
    mount();
    expect(field()).toBeNull();
    await setOpen(true);
    expect(document.activeElement).toBe(field());
  });

  it('focuses again on a reopen during the exit roll', async () => {
    // A roll that never lands, on screen, so the exit holds the row mounted.
    const pending = { finished: new Promise(() => {}), cancel: () => {} };
    HTMLElement.prototype.animate = vi.fn(() => pending as unknown as Animation);
    vi.spyOn(Element.prototype, 'getBoundingClientRect').mockReturnValue(
      { top: 10, bottom: 50, height: 40, left: 0, right: 100, width: 100, x: 0, y: 10, toJSON: () => ({}) },
    );

    const opener = document.createElement('button');
    document.body.append(opener);
    opener.focus();
    mount();
    await act(async () => { openAppSearch(); });
    await act(async () => { field()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(document.activeElement).toBe(opener);
    const leaving = field();
    expect(leaving, 'the exit roll unmounted the row').not.toBeNull();
    leaving!.blur();

    await act(async () => { openAppSearch(); });
    expect(field()).toBe(leaving);
    expect(document.activeElement).toBe(leaving);
    await act(async () => { field()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
});

for (const modifier of ['metaKey', 'ctrlKey']) {
  it(`opens app search with ${modifier}, ignores repeats, and Escape restores focus`, async () => {
    activeMenuItem.value = 'apps';
    panelOverlay.value = null;
    focusedPane.value = 'thread';
    const opener = document.createElement('button');
    document.body.append(opener);
    opener.focus();
    mount();
    const press = (repeat = false) => document.activeElement!.dispatchEvent(new KeyboardEvent('keydown', {
      key: 'Enter', [modifier]: true, altKey: true, repeat, bubbles: true, cancelable: true,
    }));
    await act(async () => { press(); });
    expect(appSearchOpen.value).toBe(true);
    expect(focusedPane.value).toBe('content');
    expect(document.activeElement).toBe(field());
    await act(async () => { press(true); });
    expect(appSearchOpen.value).toBe(true);
    opener.focus();
    await act(async () => { press(); });
    expect(document.activeElement).toBe(field());
    await act(async () => { field()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', shiftKey: true, bubbles: true })); });
    expect(appSearchOpen.value).toBe(true);
    appSearchQuery.value = 'board';
    await act(async () => { field()!.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true })); });
    expect(appSearchOpen.value).toBe(false);
    expect(appSearchQuery.value).toBe('');
    expect(document.activeElement).toBe(opener);
    opener.remove();
  });
}
