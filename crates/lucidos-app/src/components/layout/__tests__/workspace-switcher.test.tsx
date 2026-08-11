/**
 * The in-app workspace switcher's presentation, asserted on its two PURE
 * bodies. The suite has no jsdom, so this flattens the vnode tree instead of
 * rendering it: what can be checked here is which element a row becomes and
 * which classes it carries, which happens to be exactly what this surface's
 * behaviour is made of. A row that is a `<div>` cannot navigate anywhere, and
 * that is the whole guarantee behind "the workspace you are in is inert" and
 * "an unhealthy workspace is never opened from here".
 *
 * The hook-bearing wrapper (the expanded flag, the fetch, the skeleton gate) is
 * deliberately not exercised: it holds no branch this cannot see, and its two
 * real properties (no request until the row is expanded, state resets when the
 * menu closes) are structural rather than renderable. See the plan.
 *
 * The placeholder itself is the one thing in it with a checkable property, and
 * `skeletonShape` is why: the shimmer tree needs a `SkeletonProvider` and the
 * `Sk*` leaves read it through a hook, so `vnodeToText` cannot flatten it, but
 * the SHAPE it draws is pure and is exactly where the height parity lives.
 */
import { describe, it, expect } from 'vitest';
import { workspacesMenuRow, workspaceSwitcherList, skeletonShape } from '../WorkspaceSwitcher';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';
import type { WorkspaceStatus } from '../../../api/client/control';
import type { Loadable } from '../../../store/types';

const MANAGE = '/~/?pick';
const NOOP = () => {};

function ws(over: Partial<WorkspaceStatus> & { id: string }): WorkspaceStatus {
  return { name: over.id, port: 5200, health: 'healthy', autostart: false, ...over };
}

function list(
  state: Loadable<WorkspaceStatus[]>,
  { currentId = 'dev', manageHref = MANAGE as string | null } = {},
): string {
  return vnodeToText(workspaceSwitcherList({ state, currentId, manageHref, onSwitch: NOOP, onNavigate: NOOP }));
}

/** The whole flattened row that names `name`: from its own opening tag to the
 *  next row's. Rows are flat siblings in one container, so slicing between two
 *  openings is what isolates one of them, badge and check included. */
function rowFor(text: string, name: string): string {
  const at = text.indexOf(`>${name}<`);
  expect(at, `no row named "${name}"`).toBeGreaterThanOrEqual(0);
  const opens = [...text.matchAll(/<(?:div|button|a) class="brand-menu-ws-row[^"]*">/g)];
  const start = opens.filter((m) => m.index! < at).pop();
  expect(start, `row "${name}" has no opening tag`).toBeTruthy();
  const next = opens.find((m) => m.index! > at);
  return text.slice(start!.index!, next ? next.index! : text.length);
}

function row(over: Partial<Parameters<typeof workspacesMenuRow>[0]> = {}): string {
  return vnodeToText(workspacesMenuRow({
    canList: true,
    manageHref: MANAGE,
    workspaceName: 'dev',
    expanded: false,
    onToggle: NOOP,
    onNavigate: NOOP,
    ...over,
  }));
}

describe('the Workspaces row', () => {
  it('expands the list when the control plane is reachable, naming the workspace', () => {
    const text = row();
    expect(text).toContain('<button class="brand-menu-item brand-menu-ws-toggle">');
    expect(text).toContain('brand-menu-value-name');
    expect(text).toContain('dev');
    // The affordance rides in the pill, in the check's slot: beside it, it
    // would spend width the panel budgets for the workspace name.
    expect(text).toContain('brand-menu-value-chevron');
    expect(text).not.toContain('brand-menu-value-check');
  });

  it('marks itself expanded, which is what turns the chevron', () => {
    expect(row({ expanded: true })).toContain('brand-menu-ws-toggle is-expanded');
  });

  it('links out instead when there is a picker but no control plane', () => {
    // A direct engine-port page: a different origin from the gateway, so it can
    // address the picker absolutely while every relative control call would hit
    // the engine and 404. This is the row it has always had, and the shape the
    // browser suite runs against.
    const text = row({ canList: false });
    expect(text).toContain('<a class="brand-menu-item">');
    expect(text).toContain('dev');
    expect(text).toContain('brand-menu-value-check');
    expect(text).not.toContain('brand-menu-value-chevron');
  });

  it('holds the pill, marker included, before the workspace label lands', () => {
    // The chevron rides INSIDE the pill, so dropping the pill dropped the only
    // thing saying the row expands, and the row then grew both under the user's
    // finger the moment /health answered.
    const text = row({ workspaceName: null });
    expect(text).toContain('brand-menu-value-name');
    expect(text).toContain('sk-bar');
    expect(text).toContain('brand-menu-value-chevron');
  });

  it('holds it on the link-out row too, where the marker is the check', () => {
    const text = row({ canList: false, workspaceName: null });
    expect(text).toContain('sk-bar');
    expect(text).toContain('brand-menu-value-check');
  });

  it('stays a static label with no gateway at all', () => {
    // A legacy no-gateway engine. The row must still name the workspace: on
    // both mobile headers it is the only thing that does.
    const text = row({ canList: false, manageHref: null });
    expect(text).toContain('brand-menu-item-static');
    expect(text).toContain('dev');
    expect(text).not.toContain('<button');
    expect(text).not.toContain('<a ');
    expect(text).not.toContain('brand-menu-value-chevron');
  });
});

describe('the workspace list', () => {
  it('renders nothing before the listing lands, so the skeleton owns that frame', () => {
    expect(list({ status: 'not-loaded' })).toBe('');
    expect(list({ status: 'loading' })).toBe('');
  });

  it('states a failure as a failure, and still offers the way out', () => {
    const text = list({ status: 'failed', error: 'gateway unreachable' });
    expect(text).toContain('brand-menu-ws-error');
    expect(text).toContain('Could not list workspaces');
    // The reason, not just that there was one.
    expect(text).toContain('gateway unreachable');
    // Never an empty list: the gateway may have dropped while the menu was
    // open, and the picker is the recovery surface.
    expect(text).toContain('Manage workspaces');
  });

  it('makes the workspace you are in inert, and only it carries the check', () => {
    const text = list({
      status: 'loaded',
      data: [ws({ id: 'dev' }), ws({ id: 'work' })],
    });
    const current = rowFor(text, 'dev');
    expect(current).toContain('<div class="brand-menu-ws-row is-current">');
    expect(current).toContain('brand-menu-ws-check');
    const peer = rowFor(text, 'work');
    expect(peer).toContain('<button class="brand-menu-ws-row">');
    expect(peer).not.toContain('brand-menu-ws-check');
  });

  it('switches to a stopped workspace, which the gateway starts on the way in', () => {
    const text = list({
      status: 'loaded',
      data: [ws({ id: 'dev' }), ws({ id: 'asleep', health: 'unhealthy', last_error: 'not started' })],
    });
    const row = rowFor(text, 'asleep');
    expect(row).toContain('<button class="brand-menu-ws-row">');
    expect(row).toContain('ws-picker-dot-stopped');
  });

  it('never opens an unhealthy workspace: it routes to the picker instead', () => {
    // Opening into an unhealthy engine lands in a dead app shell, which is the
    // reported bug the picker's own row refuses.
    const text = list({
      status: 'loaded',
      data: [ws({ id: 'dev' }), ws({ id: 'broken', health: 'unhealthy', last_error: 'port in use' })],
    });
    const row = rowFor(text, 'broken');
    expect(row).toContain('<a class="brand-menu-ws-row is-unreachable">');
    expect(row).toContain('ws-picker-dot-unhealthy');
  });

  it('leaves an unhealthy workspace inert when there is no picker to route to', () => {
    const text = list(
      { status: 'loaded', data: [ws({ id: 'broken', health: 'unhealthy', last_error: 'port in use' })] },
      { currentId: 'dev', manageHref: null },
    );
    expect(rowFor(text, 'broken')).toContain('<div class="brand-menu-ws-row is-unreachable">');
    expect(text).not.toContain('Manage workspaces');
  });

  it('badges a workspace with unread notifications, and caps the count', () => {
    const text = list({
      status: 'loaded',
      data: [ws({ id: 'quiet', unread_count: 0 }), ws({ id: 'busy', unread_count: 3 }), ws({ id: 'loud', unread_count: 250 })],
    });
    expect(rowFor(text, 'quiet')).not.toContain('brand-menu-ws-badge');
    expect(rowFor(text, 'busy')).toContain('>3<');
    expect(rowFor(text, 'loud')).toContain('>99+<');
  });

  it('keeps the gateway listing order, so the menu and the picker agree', () => {
    const text = list({
      status: 'loaded',
      data: [ws({ id: 'work' }), ws({ id: 'dev' }), ws({ id: 'spike' })],
    });
    expect(text.indexOf('work')).toBeLessThan(text.indexOf('dev'));
    expect(text.indexOf('dev')).toBeLessThan(text.indexOf('spike'));
  });
});

describe('the loading placeholder', () => {
  /** Every `.brand-menu-ws-row` in a flattened list. All three row elements
   *  carry the class and share one `min-height`, so counting them counts the
   *  list's height in rows. */
  function rowCount(text: string): number {
    return [...text.matchAll(/<(?:div|button|a) class="brand-menu-ws-row/g)].length;
  }

  it('stands exactly as tall as the list it replaces, footer included', () => {
    // The bug this pins: the placeholder drew only workspace rows while the
    // loaded list also carries Manage workspaces, so it came up one row short
    // and the panel grew by that row at settle, pushing Refresh and Restart
    // down under the user's finger.
    const data = [ws({ id: 'dev' }), ws({ id: 'work' }), ws({ id: 'spike' })];
    const shape = skeletonShape(data.length, MANAGE);
    expect(shape.rows + (shape.manage ? 1 : 0)).toBe(rowCount(list({ status: 'loaded', data })));
  });

  it('drops the footer wherever the loaded list has none', () => {
    const data = [ws({ id: 'dev' })];
    const shape = skeletonShape(data.length, null);
    expect(shape.manage).toBe(false);
    expect(shape.rows).toBe(
      rowCount(list({ status: 'loaded', data }, { currentId: 'dev', manageHref: null })),
    );
  });

  it('guesses a short list when this device has never seen one', () => {
    // An unfolded list pushes the rows below it down, so the cheaper guess is
    // the one that moves less when it is wrong.
    expect(skeletonShape(null, MANAGE)).toEqual({ rows: 2, manage: true });
  });
});
