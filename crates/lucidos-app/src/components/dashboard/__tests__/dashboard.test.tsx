// @vitest-environment jsdom
/**
 * A dashboard (ADR 0496). Its fixed square cells take their column count from
 * the dashboard's own width, and there are three size spans. Tiles take their
 * order through CSS `order` over a DOM order that never rearranges.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

type FrameProps = {
  place: string;
  size?: string;
  visible?: boolean;
  onMenu?: (at?: { x: number; y: number }) => void;
  onFrameState?: (state: FrameState) => void;
};
const frameProps = new Map<string, FrameProps>();
/** What each mocked frame reports on mount. A frame not listed has revealed. */
const mountStates = new Map<string, FrameState>();
vi.mock('../../widgets/WidgetFrame', async () => {
  const { useLayoutEffect } = await import('preact/hooks');
  return {
    WidgetFrame: (props: FrameProps & { appId: string }) => {
      frameProps.set(props.appId, props);
      useLayoutEffect(() => props.onFrameState?.(mountStates.get(props.appId) ?? 'revealed'), []);
      return <iframe data-frame={props.appId} />;
    },
  };
});

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

import type { AppDashboardTile, DashboardTile, DashboardView, WidgetDashboardTile } from '../../../api/client/widgets';
import { dashboards } from '../../../store/dashboards';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { Dashboard, stableTileOrder } from '../Dashboard';
import { TILE_DRAG_THRESHOLD_PX } from '../useDashboardDrag';
import { DashboardRailToggle } from '../DashboardRail';
import {
  DASHBOARD_CELL_REM,
  DASHBOARD_GAP_REM,
  DASHBOARD_MIN_COLUMNS,
  DASHBOARD_PAD_X_REM,
  DASHBOARD_STEPS_PER_CELL,
  dashboardColumns,
  dashboardContentWidth,
} from '../../../utils/dashboardGeometry';
import { SIZE_SPANS } from '../DashboardTile';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';
import { DASHBOARD_SETTLE_FUSE_MS } from '../useDashboardSettled';
import { PANE_TRANSITION_MS } from '../../layout/splitHelpers';
import type { FrameState } from '../../widgets/widgetFrameState';

const openApp = vi.hoisted(() => vi.fn());
const editApp = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/apps', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/apps')>()),
  openAppById: openApp,
  openEditApp: editApp,
}));

function tile(appId: string, size: WidgetDashboardTile['size'], extra: Partial<WidgetDashboardTile> = {}): WidgetDashboardTile {
  return {
    kind: 'widget',
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

function shortcut(appId: string, extra: Partial<AppDashboardTile> = {}): AppDashboardTile {
  return { kind: 'app', app_id: appId, name: appId, size: 'quarter', ...extra };
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

  it('spans a quarter in one step, and a widget in its cells times the steps per cell', () => {
    const s = DASHBOARD_STEPS_PER_CELL;
    expect(SIZE_SPANS).toEqual({
      quarter: { columns: 1, rows: 1 },
      small: { columns: s, rows: s },
      medium: { columns: 2 * s, rows: s },
      large: { columns: 2 * s, rows: 2 * s },
    });
  });

  /** Invariant: the finer grid leaves every widget box where the cell grid put it. */
  it('draws each widget size at the box n cells and their gaps make', () => {
    const edge = DASHBOARD_CELL_REM;
    const gap = DASHBOARD_GAP_REM;
    const step = (edge - (DASHBOARD_STEPS_PER_CELL - 1) * gap) / DASHBOARD_STEPS_PER_CELL;
    const extent = (steps: number) => steps * step + (steps - 1) * gap;
    const cells = (n: number) => n * edge + (n - 1) * gap;
    expect(extent(SIZE_SPANS.small.columns)).toBeCloseTo(cells(1));
    expect(extent(SIZE_SPANS.medium.columns)).toBeCloseTo(cells(2));
    expect(extent(SIZE_SPANS.medium.rows)).toBeCloseTo(cells(1));
    expect(extent(SIZE_SPANS.large.rows)).toBeCloseTo(cells(2));
    expect(extent(SIZE_SPANS.quarter.columns)).toBeLessThan(edge / 2);
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
    mountStates.clear();
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
      ['clock', 'small', '0', 'span 2', 'span 2'],
      ['inbox', 'medium', '1', 'span 4', 'span 2'],
      ['prs', 'large', '2', 'span 4', 'span 4'],
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
    expect(dashboard.style.getPropertyValue('--dashboard-steps-per-cell')).toBe(String(DASHBOARD_STEPS_PER_CELL));
    expect(dashboard.style.getPropertyValue('--dashboard-steps')).toBe(String(DASHBOARD_MIN_COLUMNS * DASHBOARD_STEPS_PER_CELL));
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

  it('titles the head on both layouts', () => {
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(host.querySelector('.dashboard-head .dashboard-title')?.textContent).toBe('Dashboard');
    expect(host.querySelector('.dashboard-head .dashboard-edit')).not.toBeNull();
    act(() => render(<Dashboard dashboard="phone" />, host));
    expect(host.querySelector('.dashboard-head .dashboard-title')?.textContent).toBe('Dashboard');
  });

  it('an empty dashboard says so and offers Add', () => {
    setDashboard([]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(host.querySelector('.dashboard-empty')?.textContent).toContain('Nothing is on this dashboard yet');
    expect(host.querySelector('.dashboard-empty .dashboard-add')).not.toBeNull();
    expect(host.querySelector('.dashboard-empty')?.textContent).toContain('an app');
  });
});

describe('<Dashboard> settling', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    mountStates.clear();
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    vi.useRealTimers();
    render(null, host);
    host.remove();
    dashboards.value = new Map();
  });

  const grid = () => host.querySelector<HTMLElement>('.dashboard-grid:not(.loading-fade-skeleton *)')!;
  const hiding = () => grid().classList.contains('is-settling');
  const report = (appId: string, state: FrameState) => act(() => frameProps.get(appId)!.onFrameState!(state));

  /** Invariant: the grid stays hidden while a mounted frame is still covered. */
  it('hides the grid until every frame on screen has revealed', () => {
    mountStates.set('clock', 'loading');
    mountStates.set('inbox', 'loading');
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(hiding()).toBe(true);
    report('clock', 'revealed');
    expect(hiding()).toBe(true);
    report('inbox', 'revealed');
    expect(hiding()).toBe(false);
  });

  /** Invariant: an off-screen tile and an app shortcut never hold the dashboard. */
  it('is not held by an off-screen tile or an app shortcut', () => {
    mountStates.set('far', 'off-screen');
    setDashboard([tile('clock', 'small'), tile('far', 'small'), shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(hiding()).toBe(false);
  });

  /** Invariant: a tile whose place is unknown holds the dashboard. */
  it('is held by a tile the viewport observer has not reported on', () => {
    mountStates.set('clock', 'unknown');
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(hiding()).toBe(true);
  });

  /** Invariant: once settled, the grid never hides again. */
  it('stays shown when a tile added later is still loading', () => {
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(hiding()).toBe(false);
    mountStates.set('inbox', 'loading');
    act(() => setDashboard([tile('clock', 'small'), tile('inbox', 'medium')]));
    expect(hiding()).toBe(false);
    report('clock', 'loading');
    expect(hiding()).toBe(false);
  });

  /** Invariant: the fuse bounds the wait, and is cleared on unmount. */
  it('shows after the fuse while a frame is still loading', () => {
    vi.useFakeTimers();
    mountStates.set('clock', 'loading');
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    act(() => void vi.advanceTimersByTime(DASHBOARD_SETTLE_FUSE_MS - 1));
    expect(hiding()).toBe(true);
    act(() => void vi.advanceTimersByTime(1));
    expect(hiding()).toBe(false);
  });

  it('starts no fuse while hidden, and no timer outlives the dashboard', () => {
    vi.useFakeTimers();
    mountStates.set('clock', 'loading');
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" hidden />, host));
    const pending = vi.getTimerCount();
    act(() => void vi.advanceTimersByTime(DASHBOARD_SETTLE_FUSE_MS * 2));
    expect(hiding()).toBe(true);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(vi.getTimerCount()).toBeGreaterThan(pending);
    act(() => render(null, host));
    expect(vi.getTimerCount()).toBeLessThanOrEqual(pending);
  });

  /** Invariant: a hidden dashboard ignores its tiles until it is shown. */
  it('does not settle while hidden, and settles once shown', () => {
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" hidden />, host));
    expect(hiding()).toBe(true);
    report('clock', 'revealed');
    expect(hiding()).toBe(true);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(hiding()).toBe(true);
    report('clock', 'revealed');
    expect(hiding()).toBe(false);
  });

  /** Invariant: a rail sliding in reads off-screen wrongly, so it is not trusted until the slide ends. */
  it('holds an off-screen answer while a shown dashboard slides in', () => {
    vi.useFakeTimers();
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" hidden />, host));
    act(() => render(<Dashboard dashboard="desktop" />, host));
    report('clock', 'off-screen');
    expect(hiding()).toBe(true);
    act(() => { vi.advanceTimersByTime(PANE_TRANSITION_MS + 100); });
    expect(hiding()).toBe(false);
  });

  /** Invariant: the loader obeys the delay gate and is opaque when shown. */
  it('draws the skeleton in the real tiles\' sizes, only past the delay gate', () => {
    vi.useFakeTimers();
    mountStates.set('clock', 'loading');
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium'), shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    const skeleton = () => [...host.querySelectorAll<HTMLElement>('.loading-fade-skeleton .dashboard-tile')].map((t) => t.dataset.size);
    expect(skeleton()).toEqual([]);
    act(() => void vi.advanceTimersByTime(SPINNER_DELAY_MS));
    expect(skeleton()).toEqual(['small', 'medium', 'quarter']);
  });
});

describe('<Dashboard> app shortcuts', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    openApp.mockClear();
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    dashboards.value = new Map();
  });

  const shortcutTile = () => host.querySelector<HTMLElement>('[data-kind="app"]')!;

  /** Invariant: a shortcut is an icon and a name, never a running frame (ADR 0227). */
  it('draws an app as a quarter tile with its name and no frame', () => {
    setDashboard([shortcut('notes', { name: 'Notes' }), tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    const t = shortcutTile();
    expect(t.dataset.size).toBe('quarter');
    expect([t.style.gridColumn, t.style.gridRow]).toEqual(['span 1', 'span 1']);
    expect(t.querySelector('.app-shortcut-name')?.textContent).toBe('Notes');
    expect(t.querySelector('iframe')).toBeNull();
    expect(host.querySelectorAll('iframe')).toHaveLength(1);
  });

  it('names the tile by its label when it has one', () => {
    setDashboard([shortcut('notes', { name: 'Notes', label: 'Jot' })]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(shortcutTile().querySelector('.app-shortcut-name')?.textContent).toBe('Jot');
  });

  it('a tap opens the app', () => {
    setDashboard([shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    act(() => shortcutTile().click());
    expect(openApp).toHaveBeenCalledWith('notes');
  });

  it('draws no ⋯, and a right-click opens its menu and not the app', () => {
    setDashboard([shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    expect(shortcutTile().querySelectorAll('button')).toHaveLength(1);
    act(() => { shortcutTile().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); });
    const items = [...document.querySelectorAll('.thread-overflow-menu [role="menuitem"]')].map((b) => b.textContent);
    expect(items).toContain('Remove from dashboard');
    expect(openApp).not.toHaveBeenCalled();
  });

  it('its menu offers Edit app, which opens the app edit form', () => {
    editApp.mockClear();
    setDashboard([shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    act(() => { shortcutTile().dispatchEvent(new MouseEvent('contextmenu', { bubbles: true, cancelable: true })); });
    const items = [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
    expect(items.map((b) => b.textContent)).toEqual(['Open', 'Edit app', 'Remove from dashboard', 'Edit dashboard']);
    act(() => items.find((b) => b.textContent === 'Edit app')!.click());
    expect(editApp).toHaveBeenCalledWith('notes');
    expect(openApp).not.toHaveBeenCalled();
  });

  it('the menu key opens its menu, so a keyboard still reaches it', () => {
    setDashboard([shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    const button = shortcutTile().querySelector('button')!;
    act(() => { button.dispatchEvent(new KeyboardEvent('keydown', { key: 'F10', shiftKey: true, bubbles: true })); });
    const items = [...document.querySelectorAll('.thread-overflow-menu [role="menuitem"]')].map((b) => b.textContent);
    expect(items).toContain('Remove from dashboard');
  });

  it('in edit mode a tap opens nothing, and the tile offers no size pill', () => {
    setDashboard([shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    act(() => host.querySelector<HTMLButtonElement>('.dashboard-edit')!.click());
    act(() => shortcutTile().click());
    expect(openApp).not.toHaveBeenCalled();
    expect(shortcutTile().querySelector('.dashboard-tile-shield')).not.toBeNull();
    expect(shortcutTile().querySelector('.dashboard-size-pill')).toBeNull();
  });

  it('its remove badge removes the app by id', () => {
    api.removed.length = 0;
    setDashboard([shortcut('notes')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    act(() => host.querySelector<HTMLButtonElement>('.dashboard-edit')!.click());
    act(() => shortcutTile().querySelector<HTMLButtonElement>('.dashboard-tile-remove')!.click());
    expect(api.removed).toEqual([{ dashboard: 'desktop', tile: expect.objectContaining({ appId: 'notes' }) }]);
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

  /** Invariant: a shield on screen is a shield that can start a drag. A press
   *  right after the shield draws, before any passive effect has run, still
   *  grabs the tile. */
  it('a press right after edit mode draws starts a drag', async () => {
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
    host.querySelector<HTMLButtonElement>('.dashboard-edit')!.click();
    await Promise.resolve();
    const shield = tileOf('clock').querySelector<HTMLElement>('.dashboard-tile-shield')!;
    expect(shield).not.toBeNull();

    const pointer = (type: string, x: number) => shield.dispatchEvent(Object.assign(new Event(type, { bubbles: true }), {
      pointerId: 1, isPrimary: true, button: 0, clientX: x, clientY: 0,
    }));
    pointer('pointerdown', 0);
    pointer('pointermove', 2 * TILE_DRAG_THRESHOLD_PX);
    await Promise.resolve();
    expect(tileOf('clock').classList.contains('is-dragging')).toBe(true);
  });

  /** Invariant: a swap's reflow never swaps again under a stationary pointer.
   *  The tile the reflow slides under the pointer counts as passed, so a drop
   *  right after a swap commits the swap, never its reversal. */
  it('a reflow that slides another tile under the pointer does not swap straight back', () => {
    setDashboard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    edit();
    const orderNow = () => ['clock', 'inbox', 'prs'].sort((a, b) => Number(tileOf(a).style.order) - Number(tileOf(b).style.order));
    // Over `prs` before the swap; the reflow then puts `inbox` where the pointer is.
    document.elementsFromPoint = () => [tileOf(orderNow().join() === 'clock,inbox,prs' ? 'prs' : 'inbox')];
    const shield = tileOf('clock').querySelector<HTMLElement>('.dashboard-tile-shield')!;
    const pointer = (type: string, x: number) => act(() => {
      shield.dispatchEvent(Object.assign(new Event(type, { bubbles: true }), {
        pointerId: 1, isPrimary: true, button: 0, clientX: x, clientY: 0,
      }));
    });
    pointer('pointerdown', 0);
    pointer('pointermove', 2 * TILE_DRAG_THRESHOLD_PX);
    pointer('pointermove', 250);
    expect(orderNow()).toEqual(['inbox', 'prs', 'clock']);
    pointer('pointermove', 251);
    pointer('pointerup', 251);
    delete (document as Partial<Document>).elementsFromPoint;
    expect((api.arranged[0].tiles as Array<{ app_id: string }>).map((t) => t.app_id)).toEqual(['inbox', 'prs', 'clock']);
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

describe('<Dashboard> tile menu', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    host = document.createElement('div');
    document.body.appendChild(host);
    setDashboard([tile('clock', 'small')]);
    act(() => render(<Dashboard dashboard="desktop" />, host));
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    dashboards.value = new Map();
  });

  const tileBox = () => host.querySelector<HTMLElement>('[data-dashboard-tile^="clock?"]')!;
  const menuItems = () => [...document.querySelectorAll('.thread-overflow-menu [role="menuitem"]')].map((b) => b.textContent);
  const rightClick = (init: MouseEventInit = {}) => {
    const e = new MouseEvent('contextmenu', { bubbles: true, cancelable: true, clientX: 30, clientY: 40, ...init });
    act(() => { tileBox().dispatchEvent(e); });
    return e;
  };

  it('a right-click on a tile opens its menu, and does not enter edit mode', () => {
    expect(rightClick().defaultPrevented).toBe(true);
    expect(menuItems()).toContain('Edit dashboard');
    expect(host.querySelector('.dashboard-tile-shield')).toBeNull();
  });

  it('Option+right-click leaves the native menu', () => {
    expect(rightClick({ altKey: true }).defaultPrevented).toBe(false);
    expect(menuItems()).toEqual([]);
  });

  it("the frame's menu request opens the same menu, and Edit dashboard enters edit mode", () => {
    act(() => frameProps.get('clock')!.onMenu!());
    expect(menuItems()).toContain('Edit dashboard');
    const edit = [...document.querySelectorAll<HTMLButtonElement>('.thread-overflow-menu [role="menuitem"]')]
      .find((b) => b.textContent === 'Edit dashboard')!;
    act(() => edit.click());
    expect(host.querySelector('.dashboard-tile-shield')).not.toBeNull();
  });
});

describe('the rail toggle', () => {
  /** It sits on the rail's head, off the header bar, so it wears no accent tint. */
  it('wears the quiet tone', () => {
    const host = document.createElement('div');
    document.body.appendChild(host);
    threadMap.value = new Map([['home', makeThreadState('home', { meta: { title: 'Home', home: true, section: 'inbox' }, eventsLoaded: true })]]);
    act(() => render(<DashboardRailToggle />, host));
    expect(host.querySelector('.dashboard-rail-toggle')?.getAttribute('data-tone')).toBe('quiet');
    act(() => render(null, host));
    host.remove();
    threadMap.value = new Map();
  });
});
