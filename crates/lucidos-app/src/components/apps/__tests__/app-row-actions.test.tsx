// @vitest-environment jsdom
/**
 * An apps panel row acts like a thread drawer row. A pin toggle is always
 * drawn, in both states. Edit, Add to dashboard and Delete sit in the row's
 * menu: a desktop
 * row draws its ⋯ and a right-click opens the menu too, while a phone row
 * draws no ⋯ and a hold opens it. Nothing but a plain press opens the app.
 * It names no marketplace and no update: those live in the Plugins panel.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const api = vi.hoisted(() => ({ added: [] as Array<{ dashboard: string; tile: unknown }>,
  removed: [] as Array<{ dashboard: string; tile: unknown }>,
}));
vi.mock('../../../api/client/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/widgets')>()),
  addToDashboardApi: (dashboard: string, tile: unknown) => {
    api.added.push({ dashboard, tile });
    return Promise.resolve({ made_reusable: false });
  },
  removeFromDashboardApi: (dashboard: string, tile: unknown) => {
    api.removed.push({ dashboard, tile });
    return Promise.resolve({});
  },
}));

vi.mock('../../../store/actions/pinnedApps', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/pinnedApps')>()),
  togglePinApp: vi.fn(),
}));

import { AppRow } from '../AppCard';
import { isAppPinned, togglePinApp } from '../../../store/actions/pinnedApps';
import { currentDashboard, dashboards } from '../../../store/dashboards';
import { pinnedApps } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import type { App } from '../../../store/types';

const APP: App = { id: 'habit-tracker', name: 'Habit Tracker', description: 'Track habits', reveal: 'on-load', kind: 'app', reusable: false };

let host: HTMLElement;
const onOpen = vi.fn();
const onEdit = vi.fn();
const onDelete = vi.fn();

function mount(mobile = false) {
  viewportIsMobile.value = mobile;
  act(() => {
    render(<AppRow app={APP} onOpen={onOpen} onEdit={onEdit} onDelete={onDelete} />, host);
  });
}

const row = () => host.querySelector<HTMLElement>('.app-row')!;
const info = () => row().querySelector<HTMLElement>('.list-row-info')!;
const pin = () => row().querySelector<HTMLButtonElement>('button[aria-pressed]')!;
const trigger = () => host.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]');
const menuItems = () => [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
/** A row with a hint (Add to dashboard) names itself in its first span. */
const menuLabel = (el: HTMLElement) => el.querySelector('.widget-menu-label > span')?.textContent ?? el.textContent;
const menuLabels = () => menuItems().map(menuLabel);
const item = (label: string) => menuItems().find((el) => menuLabel(el) === label);
const rowButtonLabels = () =>
  [...row().querySelectorAll('button')].map((b) => b.textContent?.trim() || b.getAttribute('aria-label'));

/** Both dashboards loaded, holding these apps, so no read starts. */
function dashboardsHolding(appIds: string[]) {
  const tiles = appIds.map((app_id) => ({ kind: 'app' as const, app_id, name: app_id, size: 'quarter' as const }));
  dashboards.value = new Map([
    ['phone', { status: 'loaded', data: { dashboard: 'phone', tiles } }],
    ['desktop', { status: 'loaded', data: { dashboard: 'desktop', tiles } }],
  ]);
}

function setPinned(pinned: boolean) {
  pinnedApps.value = { status: 'loaded', data: pinned ? [{ app_id: APP.id }] : [] } as typeof pinnedApps.value;
}

/** Pin and Unpin flip the signal the row reads, as the real action does once
 *  the API call lands. */
function flipPinned(appId: string) {
  setPinned(!isAppPinned(appId));
}

function pointer(type: string, init: { clientX?: number; clientY?: number } = {}): PointerEvent {
  // jsdom has no PointerEvent constructor. `useLongPress` reads only `button`
  // and `clientX/Y`, which a MouseEvent carries.
  return new MouseEvent(type, {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: init.clientX ?? 0,
    clientY: init.clientY ?? 0,
  }) as unknown as PointerEvent;
}

/** Press, hold past the long-press threshold, lift, and send the click the
 *  browser pairs with the lift. Swallowing that click is the contract. */
function hold(target: HTMLElement, moveTo?: { x: number; y: number }) {
  act(() => {
    target.dispatchEvent(pointer('pointerdown'));
    if (moveTo) target.dispatchEvent(pointer('pointermove', { clientX: moveTo.x, clientY: moveTo.y }));
    vi.advanceTimersByTime(500);
    target.dispatchEvent(pointer('pointerup'));
    target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  });
}

function tap(target: HTMLElement) {
  act(() => {
    target.dispatchEvent(pointer('pointerdown'));
    target.dispatchEvent(pointer('pointerup'));
    target.dispatchEvent(new MouseEvent('click', { bubbles: true, cancelable: true }));
  });
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  vi.mocked(togglePinApp).mockImplementation(flipPinned);
  api.added = [];
  api.removed = [];
  dashboardsHolding([]);
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
  pinnedApps.value = { status: 'not-loaded' };
  dashboards.value = new Map();
  viewportIsMobile.value = false;
  vi.useRealTimers();
});

describe.each([['desktop', false], ['phone', true]])('the pin toggle on a %s row', (_layout, mobile) => {
  it('draws an outline pin, labelled to pin its app, while unpinned', () => {
    setPinned(false);
    mount(mobile);
    expect(pin().getAttribute('aria-label')).toBe('Pin Habit Tracker');
    expect(pin().getAttribute('aria-pressed')).toBe('false');
    expect(pin().querySelector('svg')!.getAttribute('fill')).toBe('none');
  });

  it('draws a filled accent pin, labelled to unpin its app, while pinned', () => {
    setPinned(true);
    mount(mobile);
    expect(pin().getAttribute('aria-label')).toBe('Unpin Habit Tracker');
    expect(pin().getAttribute('aria-pressed')).toBe('true');
    // The `glyph` tone paints the accent, as on a thread row.
    expect(pin().classList.contains('pin-btn')).toBe(true);
    expect(pin().getAttribute('data-tone')).toBe('glyph');
    expect(pin().querySelector('svg')!.getAttribute('fill')).toBe('currentColor');
  });

  it('is a real tab stop', () => {
    mount(mobile);
    expect(pin().tagName).toBe('BUTTON');
    expect(pin().getAttribute('type')).toBe('button');
    expect(pin().hasAttribute('tabindex')).toBe(false);
  });

  it('toggles the pin and never opens the app', () => {
    mount(mobile);
    act(() => { pin().click(); });
    expect(togglePinApp).toHaveBeenCalledWith('habit-tracker');
    expect(pin().getAttribute('aria-pressed')).toBe('true');
    expect(pin().getAttribute('aria-label')).toBe('Unpin Habit Tracker');

    act(() => { pin().click(); });
    expect(pin().getAttribute('aria-pressed')).toBe('false');
    expect(onOpen).not.toHaveBeenCalled();
    expect(menuItems()).toEqual([]);
  });

  it('replaces the read-only pinned marker after the name', () => {
    setPinned(true);
    mount(mobile);
    expect(row().querySelector('[role="img"]')).toBeNull();
    expect(row().querySelector('.app-row-title-line svg')).toBeNull();
  });

  it('draws no label beside the name', () => {
    mount(mobile);
    expect(row().querySelector('.label')).toBeNull();
  });
});

describe('a desktop row', () => {
  it('draws the pin, then the ⋯, both named for their app', () => {
    mount();
    expect(rowButtonLabels()).toEqual(['Pin Habit Tracker', 'Actions for Habit Tracker']);
    expect(trigger()!.getAttribute('aria-expanded')).toBe('false');
  });

  it('opens a menu of Edit and Delete from the ⋯, without opening the app', () => {
    mount();
    act(() => { trigger()!.click(); });
    expect(menuLabels()).toEqual(['Edit', 'Add to dashboard', 'Delete']);
    expect(trigger()!.getAttribute('aria-expanded')).toBe('true');
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('runs each item and never opens the app', () => {
    mount();
    act(() => { trigger()!.click(); });
    act(() => { item('Edit')!.click(); });
    expect(onEdit).toHaveBeenCalledTimes(1);
    expect(menuItems()).toEqual([]);

    act(() => { trigger()!.click(); });
    act(() => { item('Delete')!.click(); });
    expect(onDelete).toHaveBeenCalledTimes(1);

    expect(onOpen).not.toHaveBeenCalled();
  });

  it('adds the app to the dashboard this layout shows, and never opens it', async () => {
    mount();
    act(() => { trigger()!.click(); });
    act(() => { item('Add to dashboard')!.click(); });
    await vi.waitFor(() => expect(api.added).toHaveLength(1));
    expect(api.added).toEqual([{ dashboard: currentDashboard(), tile: { appId: 'habit-tracker', params: undefined, label: undefined } }]);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('offers Remove from dashboard once the dashboard holds the app', async () => {
    dashboardsHolding(['habit-tracker']);
    mount();
    act(() => { trigger()!.click(); });
    expect(menuLabels()).toEqual(['Edit', 'Remove from dashboard', 'Delete']);
    act(() => { item('Remove from dashboard')!.click(); });
    await vi.waitFor(() => expect(api.removed).toHaveLength(1));
    expect(api.removed[0].dashboard).toBe(currentDashboard());
  });

  it('leaves pinning to the row toggle, pinned or not', () => {
    setPinned(true);
    mount();
    act(() => { trigger()!.click(); });
    expect(menuLabels()).toEqual(['Edit', 'Add to dashboard', 'Delete']);
  });

  it('opens the same menu on a right-click on the row', () => {
    mount();
    const contextMenu = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 10, clientY: 10 });
    act(() => { info().dispatchEvent(contextMenu); });
    expect(contextMenu.defaultPrevented).toBe(true);
    expect(menuLabels()).toEqual(['Edit', 'Add to dashboard', 'Delete']);
  });

  it('leaves a right-click on the pin to the pin', () => {
    mount();
    const contextMenu = new MouseEvent('contextmenu', { bubbles: true, cancelable: true });
    act(() => { pin().dispatchEvent(contextMenu); });
    expect(menuItems()).toEqual([]);
  });

  it('arms no hold, so holding the row just opens the app', () => {
    mount();
    hold(info());
    expect(menuItems()).toEqual([]);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('opens the app on a plain click on the row', () => {
    mount();
    act(() => { info().click(); });
    expect(onOpen).toHaveBeenCalledTimes(1);
  });
});

describe('a phone row', () => {
  it('draws the pin and no ⋯', () => {
    mount(true);
    expect(rowButtonLabels()).toEqual(['Pin Habit Tracker']);
    expect(trigger()).toBeNull();
  });

  it('opens the menu of Edit and Delete on a hold, without opening the app', () => {
    mount(true);
    hold(info());
    expect(menuLabels()).toEqual(['Edit', 'Add to dashboard', 'Delete']);
    expect(onOpen).not.toHaveBeenCalled();

    act(() => { item('Edit')!.click(); });
    expect(onEdit).toHaveBeenCalledTimes(1);
    expect(onOpen).not.toHaveBeenCalled();
  });

  it('opens the app on an ordinary tap', () => {
    mount(true);
    tap(info());
    expect(menuItems()).toEqual([]);
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it('opens nothing when the finger travels: that is a scroll', () => {
    mount(true);
    hold(info(), { x: 0, y: 40 });
    expect(menuItems()).toEqual([]);
  });

  it('leaves a hold that began on the pin to the pin', () => {
    mount(true);
    hold(pin());
    expect(menuItems()).toEqual([]);
    expect(onOpen).not.toHaveBeenCalled();
  });
});
