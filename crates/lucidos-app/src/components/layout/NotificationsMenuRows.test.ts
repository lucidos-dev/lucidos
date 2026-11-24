
import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { VNode } from 'preact';
import type { WorkspaceStatus } from '../../api/client/control';
import type { RemoteGatewayView } from '../../utils/remoteWorkspaces';
import { vnodeToText } from '../chat/__tests__/vnodeToText';
vi.mock('../shared/GlyphBadge', () => import('../shared/__tests__/glyphBadgeStub'));

const platform = vi.hoisted(() => ({ isTauri: false, isStandalone: false, isMac: true }));
vi.mock('../../utils/platform', () => ({
  isTauri: () => platform.isTauri,
  isStandalone: () => platform.isStandalone,
  get isMac() {
    return platform.isMac;
  },
}));

vi.mock('../shared/Disclosure', () => import('../shared/__tests__/disclosureStub'));

const {
  notifyRows,
  notificationsMenuGroup,
} = await import('./NotificationsMenuRows');
const { countLabel } = await import('./unreadCountLabel');
type NotificationsGroupProps = import('./NotificationsMenuRows').NotificationsGroupProps;

function ws(id: string, unread?: number, health: WorkspaceStatus['health'] = 'healthy'): WorkspaceStatus {
  return { id, name: id, port: 5173, health, autostart: false, unread_count: unread };
}

function remote(gateway: string, workspaces: WorkspaceStatus[], unreachable: string | null = null): RemoteGatewayView {
  return { gateway, workspaces, unreachable };
}

function props(over: Partial<NotificationsGroupProps> = {}): NotificationsGroupProps {
  return {
    peers: [],
    remotePeers: [],
    ownUnread: 0,
    ownId: 'dev',
    ownName: 'dev',
    onOpenOwn: vi.fn(),
    onActivatePeer: vi.fn(),
    contextId: null,
    onContext: vi.fn(),
    onAlternate: vi.fn(),
    ...over,
  };
}

function vnodes(node: unknown): VNode[] {
  if (Array.isArray(node)) return node.flatMap(vnodes);
  if (!node || typeof node !== 'object' || !('type' in node)) return [];
  const v = node as VNode<{ children?: unknown }>;
  return [v, ...vnodes(v.props?.children)];
}

function rowNode(tree: unknown, id: string): VNode<Record<string, unknown>> {
  const found = vnodes(tree).find((v) => v.key === `notif:${id}`);
  expect(found, `no row keyed "notif:${id}"`).toBeTruthy();
  return found as VNode<Record<string, unknown>>;
}

function actionNode(tree: unknown, id: string): VNode<Record<string, unknown>> | undefined {
  return vnodes(tree).find((v) => v.key === `window:${id}`) as
    VNode<Record<string, unknown>> | undefined;
}

const tauri = () => {
  platform.isTauri = true;
};

beforeEach(() => {
  platform.isTauri = false;
  platform.isStandalone = false;
  platform.isMac = true;
});

describe('which workspaces become rows', () => {
  it('renders nothing at all when everything is read', () => {
    const p = props({ peers: [ws('demo', 0), ws('notes')] });
    expect(notifyRows(p)).toEqual([]);
    expect(notificationsMenuGroup(p)).toBeNull();
  });

  it('puts this workspace first, then the peers as the gateway listed them', () => {
    const rows = notifyRows(props({
      ownUnread: 2,
      peers: [ws('demo', 1), ws('notes', 3)],
    }));
    expect(rows.map((r) => [r.name, r.count, r.isSelf]))
      .toEqual([['dev', 2, true], ['demo', 1, false], ['notes', 3, false]]);
  });

  it('takes this workspace from the LIVE count, never from its polled row', () => {
    const rows = notifyRows(props({
      ownUnread: 0,
      peers: [ws('dev', 99), ws('demo', 1)],
    }));
    expect(rows.map((r) => r.name), 'our own stale row must not become a row')
      .toEqual(['demo']);
  });

  it('drops a workspace with nothing unread, and one reporting no count at all', () => {
    const rows = notifyRows(props({
      peers: [ws('demo', 0), ws('stopped', undefined, 'unhealthy'), ws('notes', 5)],
    }));
    expect(rows.map((r) => r.name)).toEqual(['notes']);
  });

  it('lists a remote gateway\'s unread workspaces last, named <name> @<gateway>', () => {
    const rows = notifyRows(props({
      ownUnread: 1,
      peers: [ws('demo', 2)],
      remotePeers: [
        remote('lab', [ws('notes', 3), ws('quiet', 0), ws('stopped', undefined, 'unhealthy')]),
      ],
    }));
    expect(rows.map((r) => [r.id, r.name, r.count, r.isSelf]))
      .toEqual([
        ['dev', 'dev', 1, true],
        ['demo', 'demo', 2, false],
        ['notes@lab', 'notes @lab', 3, false],
      ]);
  });

  it('draws no row for a remote gateway that did not answer', () => {
    const rows = notifyRows(props({
      remotePeers: [remote('lab', [ws('notes', 3)], 'timed out')],
    }));
    expect(rows).toEqual([]);
  });

  it('drops the remote row that is this very page', () => {
    const rows = notifyRows(props({
      ownId: 'notes@lab',
      ownName: 'notes @lab',
      ownUnread: 0,
      remotePeers: [remote('lab', [ws('notes', 9), ws('other', 1)])],
    }));
    expect(rows.map((r) => r.id)).toEqual(['other@lab']);
  });

  it('names this workspace even before /health has answered', () => {
    const rows = notifyRows(props({ ownUnread: 1, ownName: null }));
    expect(rows[0].name).toBe('This workspace');
  });

  it('carries this workspace on a page with no slug', () => {
    const rows = notifyRows(props({ ownId: null, ownName: 'solo', ownUnread: 4 }));
    expect(rows).toEqual([
      { id: null, name: 'solo', count: 4, isSelf: true, state: 'healthy' },
    ]);
  });

  it('carries each workspace\'s state, so the open rules can read it', () => {
    const rows = notifyRows(props({
      ownUnread: 1,
      peers: [ws('boot', 2, 'booting'), ws('sick', 3, 'unhealthy')],
    }));
    expect(rows.map((r) => r.state)).toEqual(['healthy', 'booting', 'unhealthy']);
  });
});

describe('the count on a row', () => {
  it('caps at 99+, so a backlog cannot squeeze the name beside it', () => {
    expect(countLabel(1)).toBe('1');
    expect(countLabel(99)).toBe('99');
    expect(countLabel(100)).toBe('99+');
  });
});

describe('what the group renders', () => {
  it('draws one row per workspace, with its name and count', () => {
    const text = vnodeToText(notificationsMenuGroup(props({
      ownUnread: 2,
      peers: [ws('demo', 1)],
    })));
    expect(text).toContain('dev');
    expect(text).toContain('demo');
    expect(text).toContain('class="brand-menu-ws-name"');
    expect(text).toContain('class="brand-menu-ws-badge"');
  });

  it('emits its own separator, so nothing is left behind when it is empty', () => {
    expect(vnodeToText(notificationsMenuGroup(props({ ownUnread: 1 }))))
      .toContain('class="brand-menu-separator"');
    expect(notificationsMenuGroup(props())).toBeNull();
  });
});

describe('where a tap goes', () => {
  it('routes this workspace in-app and a peer through the shared open rule', () => {
    const onOpenOwn = vi.fn();
    const onActivatePeer = vi.fn();
    const tree = notificationsMenuGroup(props({
      ownUnread: 2,
      peers: [ws('demo', 1)],
      onOpenOwn,
      onActivatePeer,
    }));

    (rowNode(tree, 'dev').props.onClick as (e: unknown) => void)({ button: 0 });
    expect(onOpenOwn).toHaveBeenCalledTimes(1);
    expect(onActivatePeer).not.toHaveBeenCalled();

    const click = { button: 0, metaKey: true };
    (rowNode(tree, 'demo').props.onClick as (e: unknown) => void)(click);
    expect(onActivatePeer).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'demo', isSelf: false }),
      click,
    );
  });

  it('answers a wheel press only where a peer has somewhere to open beside', () => {
    const tree = notificationsMenuGroup(props({
      ownUnread: 1,
      peers: [ws('demo', 1), ws('sick', 2, 'unhealthy')],
    }));
    expect(rowNode(tree, 'demo').props.onAuxClick).toBeTypeOf('function');
    expect(rowNode(tree, 'sick').props.onAuxClick, 'an unhealthy peer opens nowhere')
      .toBeUndefined();
    expect(rowNode(tree, 'dev').props.onAuxClick, 'a wheel press must not switch the view')
      .toBeUndefined();
  });

  it('never answers a wheel press with the row\'s own action', () => {
    const onActivatePeer = vi.fn();
    const tree = notificationsMenuGroup(props({ peers: [ws('demo', 1)], onActivatePeer }));
    const aux = rowNode(tree, 'demo').props.onAuxClick as (e: unknown) => void;
    const right = { button: 2, preventDefault: vi.fn() };
    aux(right);
    expect(onActivatePeer, 'the right button belongs to the action row').not.toHaveBeenCalled();
    const middle = { button: 1, preventDefault: vi.fn() };
    aux(middle);
    expect(onActivatePeer).toHaveBeenCalledTimes(1);
  });
});

describe('the right-click action row', () => {
  const two = { ownUnread: 1, peers: [ws('demo', 2)] };

  it('claims the right-click on EVERY row, so no webview menu covers the panel', () => {
    const onContext = vi.fn();
    const tree = notificationsMenuGroup(props({ ...two, onContext }));
    for (const id of ['dev', 'demo']) {
      const preventDefault = vi.fn();
      (rowNode(tree, id).props.onContextMenu as (e: unknown) => void)({ preventDefault });
      expect(preventDefault, `${id} let the native menu through`).toHaveBeenCalledTimes(1);
      expect(onContext).toHaveBeenLastCalledWith(id);
    }
  });

  it('is absent until a row is right-clicked', () => {
    expect(vnodeToText(notificationsMenuGroup(props(two))))
      .not.toContain('brand-menu-ws-action');
  });

  it('unfolds directly under the peer it belongs to, at this group\'s indent', () => {
    const text = vnodeToText(notificationsMenuGroup(props({ ...two, contextId: 'demo' })));
    expect(text).toContain(
      '<button class="brand-menu-ws-row brand-menu-ws-action brand-menu-ws-action-under-icon">',
    );
    expect(text).toContain('Open in new tab');
  });

  it('is never offered on THIS workspace\'s row, whose tap is an in-app switch', () => {
    const tree = notificationsMenuGroup(props({ ...two, contextId: 'dev' }));
    expect(actionNode(tree, 'dev')).toBeUndefined();
    expect(vnodeToText(tree)).not.toContain('brand-menu-ws-action');
  });

  it('is never offered on an unhealthy peer', () => {
    const tree = notificationsMenuGroup(props({
      peers: [ws('sick', 2, 'unhealthy')],
      contextId: 'sick',
    }));
    expect(actionNode(tree, 'sick')).toBeUndefined();
  });

  it('offers the desktop client the OTHER mode, from the same shared rule', () => {
    tauri();
    const text = vnodeToText(notificationsMenuGroup(props({ ...two, contextId: 'demo' })));
    expect(text, 'the desktop default is already a window').toContain('Switch this window');
  });

  it('hands the mode back when pressed', () => {
    const onAlternate = vi.fn();
    const tree = notificationsMenuGroup(props({ ...two, contextId: 'demo', onAlternate }));
    (actionNode(tree, 'demo')!.props.onClick as () => void)();
    expect(onAlternate).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'demo' }),
      'separate',
    );
  });
});
