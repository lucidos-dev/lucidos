/**
 * The Lucidos menu's notifications group: which rows it draws, in what order,
 * and what a tap on each one does.
 *
 * The group is a pure function, so every branch is reachable through
 * `vnodeToText` with no DOM. Same shape as `workspace-switcher.test.tsx`.
 *
 * The platform probes are mocked rather than faked through the user-agent: they
 * resolve at module load from the real environment, so a test that poked
 * `navigator` would be asserting on this machine. Same reason and same shape as
 * `utils/workspaceWindow.test.ts`, which this leans on for the modes.
 */

import { describe, it, expect, beforeEach, vi } from 'vitest';
import type { VNode } from 'preact';
import type { WorkspaceStatus } from '../../api/client/control';
import { vnodeToText } from '../chat/__tests__/vnodeToText';

const platform = vi.hoisted(() => ({ isTauri: false, isStandalone: false, isMac: true }));
vi.mock('../../utils/platform', () => ({
  isTauri: () => platform.isTauri,
  isStandalone: () => platform.isStandalone,
  get isMac() {
    return platform.isMac;
  },
}));

const {
  notifyRows,
  countLabel,
  notificationsMenuGroup,
} = await import('./NotificationsMenuRows');
type NotificationsGroupProps = import('./NotificationsMenuRows').NotificationsGroupProps;

function ws(id: string, unread?: number, health: WorkspaceStatus['health'] = 'healthy'): WorkspaceStatus {
  return { id, name: id, port: 5173, health, autostart: false, unread_count: unread };
}

function props(over: Partial<NotificationsGroupProps> = {}): NotificationsGroupProps {
  return {
    peers: [],
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

/** Every vnode in the tree, depth-first. `vnodeToText` drops props, and a
 *  handler is the whole of what a right-click or a wheel press does here. */
function vnodes(node: unknown): VNode[] {
  if (Array.isArray(node)) return node.flatMap(vnodes);
  if (!node || typeof node !== 'object' || !('type' in node)) return [];
  const v = node as VNode<{ children?: unknown }>;
  return [v, ...vnodes(v.props?.children)];
}

/** The row for workspace `id`, found by the key every row carries. */
function rowNode(tree: unknown, id: string): VNode<Record<string, unknown>> {
  const found = vnodes(tree).find((v) => v.key === `notif:${id}`);
  expect(found, `no row keyed "notif:${id}"`).toBeTruthy();
  return found as VNode<Record<string, unknown>>;
}

/** The unfolded action under workspace `id`, or undefined when none is drawn. */
function actionNode(tree: unknown, id: string): VNode<Record<string, unknown>> | undefined {
  return vnodes(tree).find((v) => v.key === `window:${id}`) as
    VNode<Record<string, unknown>> | undefined;
}

/** The packaged desktop client. */
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
    // The common case, and it must cost nothing: no empty box, and no stray
    // separator left hanging above the version row.
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
    // The gateway's listing still reports the pre-read count for a second or
    // two after an optimistic mark-read. Reading it back is what would let the
    // menu disagree with the bell about the workspace on screen.
    const rows = notifyRows(props({
      ownUnread: 0,
      peers: [ws('dev', 99), ws('demo', 1)],
    }));
    expect(rows.map((r) => r.name), 'our own stale row must not become a row')
      .toEqual(['demo']);
  });

  it('drops a workspace with nothing unread, and one reporting no count at all', () => {
    // A stopped engine reports no count (the gateway holds no DB handle), which
    // is the same running-workspaces-only rule the icon badge follows.
    const rows = notifyRows(props({
      peers: [ws('demo', 0), ws('stopped', undefined, 'unhealthy'), ws('notes', 5)],
    }));
    expect(rows.map((r) => r.name)).toEqual(['notes']);
  });

  it('names this workspace even before /health has answered', () => {
    const rows = notifyRows(props({ ownUnread: 1, ownName: null }));
    expect(rows[0].name).toBe('This workspace');
  });

  it('carries this workspace on a page with no slug', () => {
    // A legacy no-gateway engine. There are no peers to list there, so the row
    // is a plain shortcut to the notifications view.
    const rows = notifyRows(props({ ownId: null, ownName: 'solo', ownUnread: 4 }));
    expect(rows).toEqual([
      { id: null, name: 'solo', count: 4, isSelf: true, state: 'healthy' },
    ]);
  });

  // Opening a workspace is `alternateOpenMode`'s and `middleClickActivates`'
  // decision, and both ask the state, so a row that dropped it would have to
  // guess. This workspace's own row is healthy by construction: the page is
  // running inside it.
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
    // The name and count wear the switcher list's classes rather than copies.
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

    // The EVENT is handed on, not a resolved mode: only the caller can read the
    // gesture, and it reads it through `openModeForClick` like every other row.
    const click = { button: 0, metaKey: true };
    (rowNode(tree, 'demo').props.onClick as (e: unknown) => void)(click);
    expect(onActivatePeer).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'demo', isSelf: false }),
      click,
    );
  });

  // A middle press means "open this beside". On a row with nowhere to open it
  // must do nothing rather than fall back to the row's primary action.
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
    // Claimed on this workspace's row too, which unfolds nothing. Tying the
    // claim to the rows that HAVE an action is what left the switcher with a
    // row raising the native menu over the panel.
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

  // The switcher passes its own indent. Its rows lead with a status dot, and
  // these lead with the bell. One shared indent would therefore hang an action
  // off the wrong column.
  it('unfolds directly under the peer it belongs to, at this group\'s indent', () => {
    const text = vnodeToText(notificationsMenuGroup(props({ ...two, contextId: 'demo' })));
    expect(text).toContain(
      '<button class="brand-menu-ws-row brand-menu-ws-action brand-menu-ws-action-under-icon">',
    );
    // The browser wording, since the node suite is not Tauri.
    expect(text).toContain('Open in new tab');
  });

  it('is never offered on THIS workspace\'s row, whose tap is an in-app switch', () => {
    // Not a window question at all: the view it opens is already in this
    // document. A right-click there still claims the gesture, and unfolds
    // nothing.
    const tree = notificationsMenuGroup(props({ ...two, contextId: 'dev' }));
    expect(actionNode(tree, 'dev')).toBeUndefined();
    expect(vnodeToText(tree)).not.toContain('brand-menu-ws-action');
  });

  it('is never offered on an unhealthy peer', () => {
    // Opening one lands in a dead app shell, and a second window would be that
    // same shell in a new frame.
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
