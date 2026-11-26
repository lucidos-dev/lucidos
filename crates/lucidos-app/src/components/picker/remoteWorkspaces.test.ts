import { describe, expect, it } from 'vitest';
import type { ListedWorkspace } from '../../api/client/control';
import { mergeRemoteListings, remoteWorkspaceRoute, remoteWorkspaceOpenable, shownRemoteView } from './remoteWorkspaces';

const dev: ListedWorkspace = { id: 'dev', name: 'Dev', health: 'healthy' };
const notes: ListedWorkspace = { id: 'notes', name: 'Notes', health: 'unhealthy', last_error: 'not started' };

describe('mergeRemoteListings', () => {
  it('takes the rows a reachable gateway lists', () => {
    expect(mergeRemoteListings([], [{ gateway: 'cloud', state: 'reachable', workspaces: [dev] }])).toEqual([
      { gateway: 'cloud', workspaces: [dev], unreachable: null },
    ]);
  });

  it('keeps the last rows of a gateway that stopped answering, marked with the reason', () => {
    const before = [{ gateway: 'cloud', workspaces: [dev, notes], unreachable: null }];
    expect(
      mergeRemoteListings(before, [{ gateway: 'cloud', state: 'unreachable', reason: 'could not reach it' }]),
    ).toEqual([{ gateway: 'cloud', workspaces: [dev, notes], unreachable: 'could not reach it' }]);
  });

  it('shows a gateway that never answered as unreachable with no rows', () => {
    expect(mergeRemoteListings([], [{ gateway: 'box', state: 'unreachable', reason: 'down' }])).toEqual([
      { gateway: 'box', workspaces: [], unreachable: 'down' },
    ]);
  });

  it('drops a removed gateway rather than keeping its rows', () => {
    const before = [{ gateway: 'cloud', workspaces: [dev], unreachable: null }];
    expect(mergeRemoteListings(before, [])).toEqual([]);
  });
});

describe('remoteWorkspaceRoute', () => {
  it('routes a remote workspace as <slug>@<gateway>', () => {
    expect(remoteWorkspaceRoute({ gateway: 'cloud', workspaces: [dev], unreachable: null }, dev)).toBe('dev@cloud');
  });
});

describe('remoteWorkspaceOpenable', () => {
  it.each([
    ['healthy', null, null, true], ['booting', null, null, true],
    ['unhealthy', 'not started', null, false], ['unhealthy', 'failed', null, false],
    ['healthy', null, 'timeout', false], ['healthy', null, 'revoked', false],
    ['healthy', null, 'incompatible', false], ['healthy', null, 'Checking gateway', false],
  ] as const)('%s, %s, %s opens: %s', (health, last_error, unreachable, expected) => {
    expect(remoteWorkspaceOpenable({ gateway: 'cloud', workspaces: [], unreachable },
      { id: 'dev', name: 'Dev', health, last_error: last_error ?? undefined })).toBe(expected);
  });
  it.each(['timeout', 'refused', 'tls', 'revoked', 'incompatible'] as const)('retains rows for %s', (reason_kind) => {
    const rows = mergeRemoteListings([{ gateway: 'cloud', workspaces: [dev], unreachable: null }],
      [{ gateway: 'cloud', state: 'unreachable', reason: 'No answer', reason_kind }]);
    expect(rows[0].workspaces).toEqual([dev]);
    expect(remoteWorkspaceOpenable(rows[0], dev)).toBe(false);
  });
});

describe('shownRemoteView: the picker acts on the gateway its list shows', () => {
  const lab = { gateway: 'lab', workspaces: [dev], unreachable: null };

  it('shows the picked remote once its listing has loaded', () => {
    expect(shownRemoteView('lab', { status: 'loaded', data: [lab] })).toBe(lab);
  });

  it('shows this gateway while the remote listings are still loading', () => {
    expect(shownRemoteView('lab', { status: 'loading' })).toBeNull();
  });

  it('shows this gateway when the picked remote is no longer listed', () => {
    expect(shownRemoteView('gone', { status: 'loaded', data: [lab] })).toBeNull();
  });
});
