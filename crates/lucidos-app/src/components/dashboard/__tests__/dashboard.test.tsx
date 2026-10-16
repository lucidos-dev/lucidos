// @vitest-environment jsdom
/**
 * A dashboard (ADR 0496). Its fixed square cells take their column count from
 * the dashboard's own width, and there are three size spans. Tiles take their
 * order through CSS `order` over a DOM order that never rearranges.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const frameProps = new Map<string, { place: string; size?: string; visible?: boolean }>();
vi.mock('../../widgets/WidgetFrame', () => ({
  WidgetFrame: (props: { appId: string; place: string; size?: string; visible?: boolean }) => {
    frameProps.set(props.appId, props);
    return <iframe data-frame={props.appId} />;
  },
}));

const api = vi.hoisted(() => ({
  arranged: [] as Array<{ dashboard: string; tiles: unknown[] }>,
  removed: [] as Array<{ dashboard: string; tile: unknown }>,
}));
vi.mock('../../../api/client/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/widgets')>()),
  arrangeDashboardApi: (dashboard: string, tiles: unknown[]) => {
    api.arranged.push({ dashboard, tiles });
    return new Promise(() => {});
  },
  removeFromDashboardApi: (dashboard: string, tile: unknown) => {
    api.removed.push({ dashboard, tile });
    return Promise.resolve({});
  },
}));

import type { DashboardTile, DashboardView } from '../../../api/client/widgets';
import { dashboards } from '../../../store/dashboards';
import { Dashboard, stableTileOrder } from '../Dashboard';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM, DASHBOARD_MIN_COLUMNS, DASHBOARD_PAD_X_REM, dashboardColumns, dashboardContentWidth } from '../../../utils/dashboardGeometry';
import { SIZE_SPANS } from '../DashboardTile';

function tile(appId: string, size: DashboardTile['size'], extra: Partial<DashboardTile> = {}): DashboardTile {
  return {
    app_id: appId,
    name: appId,
    reusable: true,
    reveal: 'on-load',
    sizes: ['small', 'medium', 'large'],
    pinned: true,
    size,
    ...extra,
  };
}

function setDashboard(tiles: DashboardTile[]): void {
  const data: DashboardView = { dashboard: 'desktop', tiles };
  dashboards.value = new Map([['desktop', { status: 'loaded', data }]]);
}

describe('dashboard geometry', () => {
  it('fits as many cells as the width holds, never fewer than two', () => {
    const rem = 16;
    const cell = DASHBOARD_CELL_REM * rem;
    const gap = DASHBOARD_GAP_REM * rem;
    expect(dashboardColumns(4 * cell + 3 * gap, rem)).toBe(4);
    expect(dashboardColumns(4 * cell + 3 * gap - 1, rem)).toBe(3);
    expect(dashboardColumns(100, rem)).toBe(DASHBOARD_MIN_COLUMNS);
    expect(dashboardColumns(0, rem)).toBe(DASHBOARD_MIN_COLUMNS);
  });

  it('counts columns inside the dashboard\'s side padding', () => {
    const rem = 16;
    const fourCells = 4 * DASHBOARD_CELL_REM * rem + 3 * DASHBOARD_GAP_REM * rem;
    expect(dashboardContentWidth(fourCells + 2 * DASHBOARD_PAD_X_REM * rem, rem)).toBe(fourCells);
    expect(dashboardColumns(dashboardContentWidth(fourCells, rem), rem)).toBe(3);
  });

  it('scales with the UI scale, since cells are in rem', () => {
    expect(dashboardColumns(1000, 16)).toBeGreaterThan(dashboardColumns(1000, 20));
  });

  it('spans one cell, two wide, and two by two', () => {
    expect(SIZE_SPANS).toEqual({
      small: { columns: 1, rows: 1 },
      medium: { columns: 2, rows: 1 },
      large: { columns: 2, rows: 2 },
    });
  });
});

describe('stableTileOrder', () => {
  it('appends new tiles and drops removed ones without reordering the rest', () => {
    expect(stableTileOrder([], ['a', 'b', 'c'])).toEqual(['a', 'b', 'c']);
    expect(stableTileOrder(['a', 'b', 'c'], ['c', 'a', 'b'])).toEqual(['a', 'b', 'c']);
    expect(stableTileOrder(['a', 'b', 'c'], ['c', 'd', 'a'])).toEqual(['a', 'c', 'd']);
  });
});

describe('<Dashboard>', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    dashboards.value = new Map();
  });

  const tiles = () => [...host.querySelectorAll<HTMLElement>('[data-dashboard-tile]')];
  /** The tile's widget, read off the mocked frame inside it. */
  const app = (t: HTMLElement) => t.querySelector('iframe')?.dataset.frame;

  it('draws each tile at its size, with its dashboard place as CSS order', () => {
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    const drawn = tiles().map((t) => [app(t), t.dataset.size, t.style.order, t.style.gridColumn, t.style.gridRow]);
    expect(drawn).toEqual([
      ['clock', 'small', '0', 'span 1', 'span 1'],
      ['inbox', 'medium', '1', 'span 2', 'span 1'],
      ['prs', 'large', '2', 'span 2', 'span 2'],
    ]);
    expect(frameProps.get('inbox')).toMatchObject({ place: 'dashboard', size: 'medium', visible: true });
  });

  /** Invariant: reordering never moves an iframe in the DOM. */
  it('reorders by CSS order and keeps every frame element where it was', () => {
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    const before = [...host.querySelectorAll('iframe')];

    act(() => setDashboard([tile('prs', 'large'), tile('clock', 'small'), tile('inbox', 'medium')]));
    const after = [...host.querySelectorAll('iframe')];
    expect(after).toEqual(before);
    expect(after.every((frame, i) => frame === before[i])).toBe(true);
    expect(tiles().map((t) => [app(t), t.style.order])).toEqual([
      ['clock', '1'],
      ['inbox', '2'],
      ['prs', '0'],
    ]);
  });

  /** Invariant: the cell edge is one constant, read by the grid and the head. */
  it('publishes its fixed cell on the dashboard, for the grid and the head', () => {
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    const dashboard = host.querySelector<HTMLElement>('.dashboard')!;
    expect(dashboard.style.getPropertyValue('--dashboard-cell')).toBe(`${DASHBOARD_CELL_REM}rem`);
    expect(dashboard.style.getPropertyValue('--dashboard-gap')).toBe(`${DASHBOARD_GAP_REM}rem`);
    expect(dashboard.style.getPropertyValue('--dashboard-columns')).toBe(String(DASHBOARD_MIN_COLUMNS));
  });

  /** Invariant: each dashboard draws only its own tiles (ADR 0496). */
  it('draws only the tiles of the dashboard it shows', () => {
    dashboards.value = new Map([
      ['desktop', { status: 'loaded', data: { dashboard: 'desktop', tiles: [tile('clock', 'small')] } }],
      ['phone', { status: 'loaded', data: { dashboard: 'phone', tiles: [tile('inbox', 'medium')] } }],
    ]);
    act(() => render(<Dashboard dashboard="phone" />, host));
    expect(tiles().map(app)).toEqual(['inbox']);
  });

  it('a hidden dashboard tells its tiles they are not visible', () => {
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" hidden />, host));
    expect(frameProps.get('clock')?.visible).toBe(false);
  });

  it('a transparent widget draws a see-through tile', () => {
    setDashboard([tile('clock', 'small', { transparent: true }), tile('inbox', 'medium')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(tiles().map((t) => t.classList.contains('is-transparent'))).toEqual([true, false]);
  });

  /** Invariant: the desktop rail's head carries no title; the phone's does (ADR 0468). */
  it('titles the head on the phone only', () => {
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(host.querySelector('.dashboard-title')).toBeNull();
    expect(host.querySelector('.dashboard-head .dashboard-edit')).not.toBeNull();
    act(() => render(<Dashboard dashboard="phone" />, host));
    expect(host.querySelector('.dashboard-title')?.textContent).toBe('Dashboard');
  });

  it('an empty dashboard says so and offers Add', () => {
    setDashboard([]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(host.querySelector('.dashboard-empty')?.textContent).toContain('Nothing is on this dashboard yet');
    expect(host.querySelector('.dashboard-empty .dashboard-add')).not.toBeNull();
  });
});

describe('<Dashboard> in edit mode', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    api.arranged.length = 0;
    api.removed.length = 0;
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    dashboards.value = new Map();
  });

  function edit() {
    act(() => render(<Dashboard dashboard="desktop" />, host));
    act(() => host.querySelector<HTMLButtonElement>('.dashboard-edit')!.click());
  }
  const tileOf = (appId: string) => host.querySelector<HTMLElement>(`[data-dashboard-tile^="${appId}?"]`)!;

  it('shields each frame and offers only the sizes a widget declares', () => {
    setDashboard([tile('clock', 'small', { sizes: ['small', 'large'] }), tile('inbox', 'medium', { sizes: ['medium'] })]);
    edit();
    expect(host.querySelectorAll('.dashboard-tile-shield')).toHaveLength(2);
    const pill = [...tileOf('clock').querySelectorAll('.dashboard-size-pill button')].map((b) => b.textContent);
    expect(pill).toEqual(['S', 'L']);
    expect(tileOf('inbox').querySelector('.dashboard-size-pill')).toBeNull();
  });

  /** Invariant: a widget that declares no sizes comes with all three, so it
   *  offers the whole pill. */
  it('offers S, M and L on a widget that takes every size', () => {
    setDashboard([tile('inbox', 'medium')]);
    edit();
    const pill = [...tileOf('inbox').querySelectorAll('.dashboard-size-pill button')].map((b) => b.textContent);
    expect(pill).toEqual(['S', 'M', 'L']);
  });

  it('a size change writes one arrangement with every tile', () => {
    setDashboard([tile('clock', 'small', { sizes: ['small', 'large'] }), tile('inbox', 'medium')]);
    edit();
    act(() => tileOf('clock').querySelector<HTMLButtonElement>('.dashboard-size-pill button[aria-label="large"]')!.click());
    expect(api.arranged).toEqual([{
      dashboard: 'desktop',
      tiles: [{ app_id: 'clock', size: 'large' }, { app_id: 'inbox', size: 'medium' }],
    }]);
    expect(tileOf('clock').dataset.size).toBe('large');
  });

  it('Alt and an arrow moves a tile one place and writes one arrangement', () => {
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    edit();
    const frames = [...host.querySelectorAll('iframe')];
    const shield = tileOf('clock').querySelector<HTMLElement>('.dashboard-tile-shield')!;
    act(() => { shield.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true })); });
    expect(api.arranged).toHaveLength(1);
    expect((api.arranged[0].tiles as Array<{ app_id: string }>).map((t) => t.app_id)).toEqual(['inbox', 'clock', 'prs']);
    expect([...host.querySelectorAll('iframe')]).toEqual(frames);

    act(() => { shield.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })); });
    expect(api.arranged).toHaveLength(1);
  });

  it('the minus badge removes the tile from this dashboard', () => {
    setDashboard([tile('clock', 'small')]);
    edit();
    act(() => tileOf('clock').querySelector<HTMLButtonElement>('.dashboard-tile-remove')!.click());
    expect(api.removed).toEqual([{ dashboard: 'desktop', tile: expect.objectContaining({ appId: 'clock' }) }]);
  });

  it('Done leaves edit mode', () => {
    setDashboard([tile('clock', 'small')]);
    edit();
    act(() => host.querySelector<HTMLButtonElement>('.dashboard-done')!.click());
    expect(host.querySelector('.dashboard-tile-shield')).toBeNull();
    expect(host.querySelector('.dashboard-edit')).not.toBeNull();
  });
});
