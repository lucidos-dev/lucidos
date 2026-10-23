// @vitest-environment jsdom
/**
 * A dashboard's Add lists the installed apps under the reusable widgets, and a
 * pick adds an app shortcut. A thread's Add widget lists no app, and a dashboard
 * leaves out an app it already shows.
 */
import { describe, it, expect, afterEach, beforeEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const api = vi.hoisted(() => ({
  added: [] as Array<{ dashboard: string; tile: unknown }>,
  failWith: null as string | null,
}));
vi.mock('../../../api/client/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/widgets')>()),
  addToDashboardApi: (dashboard: string, tile: unknown) => {
    api.added.push({ dashboard, tile });
    return api.failWith ? Promise.reject(new Error(api.failWith)) : Promise.resolve({ made_reusable: false });
  },
}));
const toasts = vi.hoisted(() => [] as string[]);
vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showToast: (message: string) => { toasts.push(message); },
}));

import { WidgetPickerItems } from '../WidgetPickerItems';
import { dashboards } from '../../../store/dashboards';
import { appsList } from '../../../store/store';
import { reusableWidgets } from '../../../store/widgets';
import type { App } from '../../../store/types';

const clock = { id: 'clock', name: 'Clock', description: 'The time', kind: 'widget', reusable: true, reveal: 'on-load' } as App;
const tracker = { id: 'habit-tracker', name: 'Habit Tracker', description: 'Track habits', kind: 'app', reusable: false, reveal: 'on-load' } as App;
const notes = { id: 'notes', name: 'Notes', description: '', kind: 'app', reusable: false, reveal: 'on-load' } as App;

let host: HTMLElement;
const run = (fn: () => void) => () => fn();
const ids = (attr: string) => [...host.querySelectorAll<HTMLElement>(`[${attr}]`)].map((el) => el.getAttribute(attr));

function mount(target: Parameters<typeof WidgetPickerItems>[0]['target']): void {
  act(() => { render(<WidgetPickerItems run={run} target={target} />, host); });
}

function holding(appIds: string[]): void {
  const tiles = appIds.map((app_id) => ({ kind: 'app' as const, app_id, name: app_id, size: 'quarter' as const }));
  dashboards.value = new Map([['desktop', { status: 'loaded', data: { dashboard: 'desktop', tiles } }]]);
}

beforeEach(() => {
  api.added = [];
  api.failWith = null;
  toasts.length = 0;
  host = document.createElement('div');
  document.body.appendChild(host);
  reusableWidgets.value = { status: 'loaded', data: [clock] };
  appsList.value = { status: 'loaded', data: [tracker, notes] };
  holding([]);
});

afterEach(() => {
  act(() => { render(null, host); });
  host.remove();
  dashboards.value = new Map();
  appsList.value = { status: 'not-loaded' };
  reusableWidgets.value = { status: 'not-loaded' };
});

describe('the dashboard picker', () => {
  it('lists the widgets, then the apps under an Apps note', () => {
    mount({ dashboard: 'desktop' });
    expect(ids('data-add-widget')).toEqual(['clock']);
    expect(ids('data-add-app')).toEqual(['habit-tracker', 'notes']);
    expect(host.textContent).toContain('Apps');
  });

  it('adds an app as a shortcut, carrying no params', async () => {
    mount({ dashboard: 'desktop' });
    act(() => { host.querySelector<HTMLElement>('[data-add-app="habit-tracker"]')!.click(); });
    await vi.waitFor(() => expect(api.added).toHaveLength(1));
    expect(api.added).toEqual([{ dashboard: 'desktop', tile: { appId: 'habit-tracker' } }]);
    await vi.waitFor(() => expect(toasts).toEqual(['Added "Habit Tracker" to the desktop dashboard.']));
  });

  it('leaves out an app the dashboard already shows', () => {
    holding(['notes']);
    mount({ dashboard: 'desktop' });
    expect(ids('data-add-app')).toEqual(['habit-tracker']);
  });

  it('draws no Apps note once every app is on the dashboard', () => {
    holding(['habit-tracker', 'notes']);
    mount({ dashboard: 'desktop' });
    expect(ids('data-add-app')).toEqual([]);
    expect(host.textContent).not.toContain('Apps');
  });

  it('names an app, not a widget, when adding it fails', async () => {
    api.failWith = 'engine down';
    mount({ dashboard: 'desktop' });
    act(() => { host.querySelector<HTMLElement>('[data-add-app="notes"]')!.click(); });
    await vi.waitFor(() => expect(toasts).toHaveLength(1));
    expect(toasts[0]).toBe('Couldn\'t add app "Notes" to the desktop dashboard: engine down');
  });

  it('says so when the apps list could not be read', () => {
    appsList.value = { status: 'failed', error: 'engine down' };
    mount({ dashboard: 'desktop' });
    expect(host.querySelector('[role="alert"]')?.textContent).toBe('Couldn\'t list apps: engine down');
  });
});

describe('a picker row description', () => {
  it('shows as a tooltip, not as text in the row', () => {
    mount({ dashboard: 'desktop' });
    const row = (selector: string) => host.querySelector<HTMLElement>(selector)!;
    expect(row('[data-add-widget="clock"]').getAttribute('data-tooltip')).toBe('The time');
    expect(row('[data-add-app="habit-tracker"]').getAttribute('data-tooltip')).toBe('Track habits');
    expect(row('[data-add-app="notes"]').hasAttribute('data-tooltip')).toBe(false);
    expect(host.textContent).not.toContain('Track habits');
  });
});

describe('a thread picker', () => {
  it('lists no app', () => {
    mount({ threadId: 't' });
    expect(ids('data-add-widget')).toEqual(['clock']);
    expect(ids('data-add-app')).toEqual([]);
  });
});
