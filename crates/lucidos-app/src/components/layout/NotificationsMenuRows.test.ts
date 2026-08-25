/**
 * The Lucidos menu's notifications group: which rows it draws, in what order,
 * and what a tap on each one does.
 *
 * The group is a pure function, so every branch is reachable through
 * `vnodeToText` with no DOM. Same shape as `workspace-switcher.test.tsx`.
 */

import { describe, it, expect, vi } from 'vitest';
import type { WorkspaceStatus } from '../../api/client/control';
import { vnodeToText } from '../chat/__tests__/vnodeToText';
import {
  notifyRows,
  countLabel,
  notificationsMenuGroup,
  type NotificationsGroupProps,
} from './NotificationsMenuRows';

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
    onOpenPeer: vi.fn(),
    ...over,
  };
}

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
    expect(rows).toEqual([{ id: null, name: 'solo', count: 4, isSelf: true }]);
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
  it('routes this workspace in-app and a peer across workspaces', () => {
    const onOpenOwn = vi.fn();
    const onOpenPeer = vi.fn();
    const p = props({ ownUnread: 2, peers: [ws('demo', 1)], onOpenOwn, onOpenPeer });
    const group = notificationsMenuGroup(p) as { props: { children: unknown[] } };
    const [list] = group.props.children as [{ props: { children: { props: { onClick: () => void } }[] } }];
    const [selfRow, peerRow] = list.props.children;

    selfRow.props.onClick();
    expect(onOpenOwn).toHaveBeenCalledTimes(1);
    expect(onOpenPeer).not.toHaveBeenCalled();

    peerRow.props.onClick();
    expect(onOpenPeer).toHaveBeenCalledWith(
      expect.objectContaining({ id: 'demo', isSelf: false }),
    );
  });
});
