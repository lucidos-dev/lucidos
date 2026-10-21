// @vitest-environment jsdom
/**
 * A built-in widget is read-only (ADR 0415), so its menu offers no reuse
 * change. Every other item stays. Add to dashboard is offered in every thread
 * (ADR 0496).
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';

const api = vi.hoisted(() => ({ added: [] as Array<{ dashboard: string; tile: unknown }> }));
vi.mock('../../../api/client/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/widgets')>()),
  addToDashboardApi: (dashboard: string, tile: unknown) => {
    api.added.push({ dashboard, tile });
    return Promise.resolve({ made_reusable: false });
  },
}));

import { widgetMenuItems, type WidgetMenuTarget } from '../WidgetMenu';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { currentDashboard, dashboards } from '../../../store/dashboards';
import type { DashboardTile } from '../../../api/client/widgets';

let host: HTMLElement | null = null;
function unmount(): void {
  if (host) render(null, host);
  host?.remove();
  host = null;
}
afterEach(() => {
  unmount();
  threadMap.value = new Map();
  dashboards.value = new Map();
});

/** Both dashboards loaded, each holding these widgets, so no read starts. */
function holding(apps: string[]): void {
  const tiles: DashboardTile[] = apps.map((app_id) => (
    { kind: 'widget', app_id, name: app_id, reusable: true, reveal: 'on-load', sizes: ['medium'], pinned: true, size: 'medium' }
  ));
  dashboards.value = new Map([
    ['phone', { status: 'loaded', data: { dashboard: 'phone', tiles } }],
    ['desktop', { status: 'loaded', data: { dashboard: 'desktop', tiles } }],
  ]);
}

function hints(): string[] {
  return [...(host?.querySelectorAll('.widget-menu-hint') ?? [])].map((el) => el.textContent ?? '');
}

function labels(target: Partial<WidgetMenuTarget>): string[] {
  unmount();
  host = document.createElement('div');
  document.body.appendChild(host);
  const ctx = { run: (fn: () => void) => () => fn(), anchor: null };
  render(widgetMenuItems(ctx, {
    threadId: 't-1', appId: 'w', name: 'W', reusable: true, builtIn: false, pinned: false, ...target,
  }), host);
  return [...host.querySelectorAll('.widget-menu-label > span:first-child')].map((el) => el.textContent ?? '');
}

describe('widgetMenuItems', () => {
  it('offers Stop reusing on a reusable workspace widget', () => {
    expect(labels({})).toContain('Stop reusing');
  });

  it('offers no reuse change on a built-in widget', () => {
    const items = labels({ builtIn: true });
    expect(items).not.toContain('Stop reusing');
    expect(items).not.toContain('Make reusable');
    expect(items).toEqual(expect.arrayContaining(['Open in Canvas', 'Make app', 'Pin to shelf']));
  });

  /** Home takes no pins (ADR 0496), so it offers Add to dashboard alone. */
  it('offers no shelf pin in Home, only Add to dashboard', () => {
    threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true } })]]);
    holding([]);
    const items = labels({ threadId: 'home' });
    expect(items).toContain('Add to dashboard');
    expect(items).not.toContain('Pin to shelf');
    expect(items).not.toContain('Unpin from shelf');
  });

  /** Invariant: Add to dashboard targets the dashboard this layout shows, and
   *  turns into Remove once that dashboard holds the instance. */
  it('offers Add to dashboard beside Pin to shelf, until the dashboard holds the instance', () => {
    holding([]);
    expect(labels({})).toEqual(expect.arrayContaining(['Pin to shelf', 'Add to dashboard']));
    holding(['w']);
    expect(labels({})).toContain('Remove from dashboard');
    expect(labels({})).not.toContain('Add to dashboard');
  });

  it('adds to the dashboard of the current layout, and names it in its hint', async () => {
    holding([]);
    labels({ params: { clip: 'x.mp3' }, label: 'Clip' });
    expect(hints()).toContain(`A tile on the ${currentDashboard()} dashboard`);
    const add = [...host!.querySelectorAll<HTMLButtonElement>('.widget-menu-item')]
      .find((b) => b.textContent?.startsWith('Add to dashboard'))!;
    add.click();
    await Promise.resolve();
    expect(api.added).toEqual([{ dashboard: currentDashboard(), tile: { appId: 'w', params: { clip: 'x.mp3' }, label: 'Clip' } }]);
  });

  /** Invariant: the row never offers an action its unread dashboard may contradict. */
  it('keeps Add to dashboard inert, and says why, while its dashboard is unread', async () => {
    api.added = [];
    dashboards.value = new Map([[currentDashboard(), { status: 'failed', error: 'engine down' }]]);
    labels({});
    const add = [...host!.querySelectorAll<HTMLButtonElement>('.widget-menu-item')]
      .find((b) => b.textContent?.startsWith('Add to dashboard'))!;
    expect(add.getAttribute('aria-disabled')).toBe('true');
    expect(hints()).toContain(`Couldn't read the ${currentDashboard()} dashboard: engine down`);
    add.click();
    await Promise.resolve();
    expect(api.added).toEqual([]);
  });
});
