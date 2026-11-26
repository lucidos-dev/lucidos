import { describe, it, expect, vi } from 'vitest';
import type { VNode } from 'preact';
vi.mock('../../shared/Disclosure', () => import('../../shared/__tests__/disclosureStub'));

import { workspacesMenuRow, workspaceSwitcherList, skeletonShape, initiallyExpanded } from '../WorkspaceSwitcher';
import type { SwitcherListProps } from '../WorkspaceSwitcher';
import { vnodeToText } from '../../chat/__tests__/vnodeToText';
import type { WorkspaceStatus } from '../../../api/client/control';
import type { Loadable } from '../../../store/types';
import type { RemoteGatewayView } from '../../../utils/remoteWorkspaces';

const MANAGE = '/~/?pick';
const NOOP = () => {};
const CLOUD: RemoteGatewayView = {
  gateway: 'cloud',
  host: 'cloud.example',
  workspaces: [{ id: 'notes', name: 'notes', health: 'healthy' }],
  unreachable: null,
};

function ws(over: Partial<WorkspaceStatus> & { id: string }): WorkspaceStatus {
  return { name: over.id, port: 5200, health: 'healthy', autostart: false, ...over };
}

function listVNode(
  state: Loadable<WorkspaceStatus[]>,
  over: Partial<Omit<SwitcherListProps, 'state'>> = {},
) {
  return workspaceSwitcherList({
    state,
    currentId: 'dev',
    manageHref: MANAGE,
    contextId: null,
    onActivate: NOOP,
    onNavigate: NOOP,
    onContext: NOOP,
    onAlternate: NOOP,
    remote: [],
    remoteError: null,
    order: [],
    onOpenRemote: NOOP,
    ...over,
  });
}

function list(
  state: Loadable<WorkspaceStatus[]>,
  over: Partial<Omit<SwitcherListProps, 'state'>> = {},
): string {
  return vnodeToText(listVNode(state, over));
}

function vnodes(node: unknown): VNode[] {
  if (Array.isArray(node)) return node.flatMap(vnodes);
  if (!node || typeof node !== 'object' || !('type' in node)) return [];
  const v = node as VNode<{ children?: unknown }>;
  return [v, ...vnodes(v.props?.children)];
}

function rowNode(node: unknown, id: string): VNode<Record<string, unknown>> {
  const found = vnodes(node).find((v) => v.key === id);
  expect(found, `no row keyed "${id}"`).toBeTruthy();
  return found as VNode<Record<string, unknown>>;
}

function rowFor(text: string, name: string): string {
  const at = text.indexOf(`>${name}<`);
  expect(at, `no row named "${name}"`).toBeGreaterThanOrEqual(0);
  const opens = [...text.matchAll(/<(?:div|button|a) class="brand-menu-ws-row[^"]*">/g)];
  const start = opens.filter((m) => m.index! < at).pop();
  expect(start, `row "${name}" has no opening tag`).toBeTruthy();
  const next = opens.find((m) => m.index! > at);
  return text.slice(start!.index!, next ? next.index! : text.length);
}

function classesAfter(text: string, name: string): string {
  const at = text.indexOf(`>${name}<`);
  expect(at, `no row named "${name}"`).toBeGreaterThanOrEqual(0);
  const opens = [...text.matchAll(/<(?:div|button|a) class="(brand-menu-ws-row[^"]*)">/g)];
  return opens.find((m) => m.index! > at)?.[1] ?? '';
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
    expect(text).toContain('brand-menu-value-chevron');
    expect(text).not.toContain('brand-menu-value-check');
  });

  it('marks itself expanded, which is what turns the chevron', () => {
    expect(row({ expanded: true })).toContain('brand-menu-ws-toggle is-expanded');
  });

  it('links out instead when there is a picker but no control plane', () => {
    const text = row({ canList: false });
    expect(text).toContain('<a class="brand-menu-item">');
    expect(text).toContain('dev');
    expect(text).toContain('brand-menu-value-check');
    expect(text).not.toContain('brand-menu-value-chevron');
  });

  it('keeps the pill and its chevron before the workspace label lands', () => {
    const text = row({ workspaceName: null });
    expect(text).toContain('brand-menu-value');
    expect(text).toContain('brand-menu-value-chevron');
  });

  it('keeps it on the link-out row too, where the marker is the check', () => {
    const text = row({ canList: false, workspaceName: null });
    expect(text).toContain('brand-menu-value');
    expect(text).toContain('brand-menu-value-check');
  });

  it('draws no placeholder in the empty pill, on either row', () => {
    expect(row({ workspaceName: null })).not.toContain('sk-bar');
    expect(row({ canList: false, workspaceName: null })).not.toContain('sk-bar');
  });

  it('trails the marker, so a late label cannot drag it sideways', () => {
    const chevron = row();
    expect(chevron.indexOf('dev')).toBeLessThan(chevron.indexOf('brand-menu-value-chevron'));
    const check = row({ canList: false });
    expect(check.indexOf('dev')).toBeLessThan(check.indexOf('brand-menu-value-check'));
  });

  it('stays a static label with no gateway at all', () => {
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
    expect(text).toContain('gateway unreachable');
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

  it('draws no unread count, since the notifications group above already does', () => {
    const text = list({
      status: 'loaded',
      data: [ws({ id: 'dev', unread_count: 4 }), ws({ id: 'busy', unread_count: 3 })],
    });
    expect(text).not.toContain('brand-menu-ws-badge');
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
  function rowCount(text: string): number {
    return [...text.matchAll(/<(?:div|button|a) class="brand-menu-ws-row/g)].length;
  }

  it('stands exactly as tall as the list it replaces, footer included', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'work' }), ws({ id: 'spike' })];
    const shape = skeletonShape(data.length, 0, MANAGE);
    expect(shape.rows + (shape.manage ? 1 : 0)).toBe(rowCount(list({ status: 'loaded', data })));
  });

  it('counts the cached remote rows, which the loaded list draws too', () => {
    const data = [ws({ id: 'dev' })];
    const shape = skeletonShape(data.length, CLOUD.workspaces.length, MANAGE);
    expect(shape.rows + 1).toBe(rowCount(list({ status: 'loaded', data }, { remote: [CLOUD] })));
  });

  it('drops the footer wherever the loaded list has none', () => {
    const data = [ws({ id: 'dev' })];
    const shape = skeletonShape(data.length, 0, null);
    expect(shape.manage).toBe(false);
    expect(shape.rows).toBe(
      rowCount(list({ status: 'loaded', data }, { currentId: 'dev', manageHref: null })),
    );
  });

  it('guesses a short list when this device has never seen one', () => {
    expect(skeletonShape(null, 0, MANAGE)).toEqual({ rows: 2, manage: true });
  });
});

describe('remote workspaces in the list', () => {
  function rowKeys(node: unknown): string[] {
    return vnodes(node)
      .filter((v) => typeof v.key === 'string' && /\bbrand-menu-ws-row\b/.test(String((v.props as { class?: unknown }).class ?? '')))
      .map((v) => String(v.key))
      .filter((key) => !key.startsWith('window:'));
  }

  it('draws local rows first, then remote ones, when no order is stored', () => {
    const node = listVNode({ status: 'loaded', data: [ws({ id: 'dev' }), ws({ id: 'work' })] }, { remote: [CLOUD] });
    expect(rowKeys(node)).toEqual(['dev', 'work', 'notes@cloud']);
  });

  it('follows the stored workspace order across local and remote rows', () => {
    const node = listVNode(
      { status: 'loaded', data: [ws({ id: 'dev' }), ws({ id: 'work' })] },
      { remote: [CLOUD], order: ['notes@cloud', 'work'] },
    );
    expect(rowKeys(node)).toEqual(['notes@cloud', 'work', 'dev']);
  });

  it('wears the local row shape with a quiet gateway tag, and no heading', () => {
    const node = listVNode({ status: 'loaded', data: [ws({ id: 'dev' })] }, { remote: [CLOUD] });
    const tag = vnodes(rowNode(node, 'notes@cloud')).find((v) => (v.props as { class?: unknown }).class === 'brand-menu-ws-gateway');
    expect(vnodeToText(tag)).toContain('@cloud');
    expect(String(rowNode(node, 'notes@cloud').props.class)).toBe('brand-menu-ws-row');
    const text = vnodeToText(node);
    expect(text).not.toContain('remote');
    expect(text).not.toContain('cloud.example');
  });

  it('opens through this gateway\'s relay route', () => {
    const r = rowNode(listVNode({ status: 'loaded', data: [] }, { remote: [CLOUD] }), 'notes@cloud');
    expect(r.type).toBe('a');
    expect(r.props.href).toBe('/notes@cloud/');
  });

  it('marks the remote workspace this page serves, not its local namesake', () => {
    const node = listVNode(
      { status: 'loaded', data: [ws({ id: 'notes' })] },
      { currentId: 'notes@cloud', remote: [CLOUD] },
    );
    expect(rowNode(node, 'notes@cloud').props['aria-current']).toBe('page');
    expect(rowNode(node, 'notes').props['aria-current']).toBeUndefined();
  });

  it('keeps an unreachable gateway\'s rows greyed and shut, saying why', () => {
    const silent = { ...CLOUD, unreachable: 'Gateway did not answer' };
    const r = rowNode(listVNode({ status: 'loaded', data: [] }, { remote: [silent] }), 'notes@cloud');
    expect(r.type).toBe('div');
    expect(r.props.class).toContain('is-shut');
    expect(r.props['data-tooltip']).toContain('Gateway did not answer');
  });

  it('still draws remote rows beside a failed local listing', () => {
    const node = listVNode({ status: 'failed', error: 'gateway down' }, { remote: [CLOUD] });
    expect(rowKeys(node)).toEqual(['notes@cloud']);
  });

  it('states a remote listing failure under the rows', () => {
    const text = list({ status: 'loaded', data: [ws({ id: 'dev' })] }, { remoteError: 'relay refused' });
    expect(text).toContain('relay refused');
  });
});

describe('the remembered expand state', () => {
  it('reopens unfolded where the list can be fetched', () => {
    expect(initiallyExpanded(true, true)).toBe(true);
    expect(initiallyExpanded(true, false)).toBe(false);
  });

  it('never unfolds where the control plane is out of reach', () => {
    expect(initiallyExpanded(false, true)).toBe(false);
  });
});

describe('the right-click action row', () => {
  const two = { status: 'loaded' as const, data: [ws({ id: 'dev' }), ws({ id: 'work' })] };

  it('is absent until a row is right-clicked', () => {
    expect(list(two)).not.toContain('brand-menu-ws-action');
  });

  it('unfolds directly under the workspace it belongs to, and under nothing else', () => {
    const text = list(two, { contextId: 'work' });
    expect(classesAfter(text, 'work')).toContain('brand-menu-ws-action');
    expect(classesAfter(text, 'dev')).not.toContain('brand-menu-ws-action');
  });

  it('is a plain row of this menu, never a panel over it', () => {
    expect(list(two, { contextId: 'work' })).toContain(
      '<button class="brand-menu-ws-row brand-menu-ws-action brand-menu-ws-action-under-dot">',
    );
  });

  it('says what pressing it does, in this platform\'s words', () => {
    expect(list(two, { contextId: 'work' })).toContain('Open in new tab');
  });

  it('is offered on the workspace you are already in', () => {
    expect(classesAfter(list(two, { contextId: 'dev' }), 'dev'))
      .toContain('brand-menu-ws-action');
  });

  it('is never offered on an unhealthy workspace', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'work', health: 'unhealthy', last_error: 'boom' })];
    expect(list({ status: 'loaded', data }, { contextId: 'work' }))
      .not.toContain('brand-menu-ws-action');
  });

  it('takes a key no workspace slug can reach', () => {
    const data = [ws({ id: 'work' }), ws({ id: 'work-window' })];
    const keys = vnodes(listVNode({ status: 'loaded', data }, { contextId: 'work' }))
      .map((v) => v.key)
      .filter((k) => k != null);
    expect(keys.length).toBeGreaterThan(2);
    expect(new Set(keys).size).toBe(keys.length);
  });
});

describe('the right-click itself', () => {
  const two = { status: 'loaded' as const, data: [ws({ id: 'dev' }), ws({ id: 'work' })] };

  function rightClick(node: unknown, id: string) {
    const handler = rowNode(node, id).props.onContextMenu as
      | ((e: { preventDefault(): void }) => void)
      | undefined;
    const preventDefault = vi.fn();
    handler?.({ preventDefault });
    return { claimed: preventDefault.mock.calls.length > 0, wired: handler !== undefined };
  }

  it('takes the gesture off the browser, whose own menu would cover the panel', () => {
    const onContext = vi.fn();
    const onActivate = vi.fn();
    const node = listVNode(two, { onContext, onActivate });
    expect(rightClick(node, 'work').claimed).toBe(true);
    expect(onContext).toHaveBeenCalledWith('work');
    expect(onActivate).not.toHaveBeenCalled();
  });

  it('is wired on the workspace you are already in', () => {
    expect(rightClick(listVNode(two), 'dev').wired).toBe(true);
  });

  it('is left to the browser on an unhealthy row, which offers nothing', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'work', health: 'unhealthy', last_error: 'boom' })];
    expect(rightClick(listVNode({ status: 'loaded', data }), 'work').wired).toBe(false);
  });
});

describe('activating a row', () => {
  const two = { status: 'loaded' as const, data: [ws({ id: 'dev' }), ws({ id: 'work' })] };

  function handler(node: unknown, id: string, name: 'onClick' | 'onAuxClick') {
    return rowNode(node, id).props[name] as ((e: unknown) => void) | undefined;
  }

  it('hands the EVENT on, so the caller can read the gesture off it', () => {
    const onActivate = vi.fn();
    const node = listVNode(two, { onActivate });
    const event = { button: 0, metaKey: true };
    handler(node, 'work', 'onClick')?.(event);
    expect(onActivate).toHaveBeenCalledWith(two.data[1], event);
  });

  it('answers the middle button, which dispatches no click of its own', () => {
    const onActivate = vi.fn();
    const node = listVNode(two, { onActivate });
    const preventDefault = vi.fn();
    handler(node, 'work', 'onAuxClick')?.({ button: 1, preventDefault });
    expect(onActivate).toHaveBeenCalledTimes(1);
    expect(preventDefault).toHaveBeenCalled();
  });

  it('leaves the right button to the action row', () => {
    const onActivate = vi.fn();
    const node = listVNode(two, { onActivate });
    const preventDefault = vi.fn();
    handler(node, 'work', 'onAuxClick')?.({ button: 2, preventDefault });
    expect(onActivate).not.toHaveBeenCalled();
    expect(preventDefault).not.toHaveBeenCalled();
  });

  it('leaves the current and unhealthy rows with nothing to activate', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'broken', health: 'unhealthy', last_error: 'boom' })];
    const node = listVNode({ status: 'loaded', data });
    expect(handler(node, 'dev', 'onClick')).toBeUndefined();
    expect(handler(node, 'dev', 'onAuxClick')).toBeUndefined();
    expect(handler(node, 'broken', 'onAuxClick')).toBeUndefined();
  });

  it('routes an unhealthy row to the picker, and never to the workspace itself', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'broken', health: 'unhealthy', last_error: 'port in use' })];
    const node = listVNode({ status: 'loaded', data }, { manageHref: MANAGE });
    expect(rowNode(node, 'broken').props.href).toBe(MANAGE);
    expect(vnodeToText(rowNode(node, 'broken'))).not.toContain('/broken/');
  });
});

describe('the rows that cannot be opened carry a menu role', () => {
  it('keeps the current and unhealthy rows as disabled menuitems, never role none', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'broken', health: 'unhealthy', last_error: 'port in use' })];
    const node = listVNode({ status: 'loaded', data }, { manageHref: null });
    for (const id of ['dev', 'broken']) {
      const props = rowNode(node, id).props;
      expect(props.role, `row "${id}"`).toBe('menuitem');
      expect(props['aria-disabled'], `row "${id}"`).toBe('true');
    }
  });
});

describe('an unhealthy row names its fault to assistive tech', () => {
  it('puts the last error into the row label, not only into a hover tooltip', () => {
    const data = [ws({ id: 'dev' }), ws({ id: 'broken', health: 'unhealthy', last_error: 'port in use' })];
    const label = rowNode(listVNode({ status: 'loaded', data }, { manageHref: MANAGE }), 'broken').props['aria-label'] as string;
    expect(label).toContain('port in use');
    expect(label).toContain('broken');
  });
});
