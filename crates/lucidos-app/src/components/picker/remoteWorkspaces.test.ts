import { describe, expect, it } from 'vitest';
import type { ListedWorkspace } from '../../api/client/control';
import { mergeRemoteListings, remoteWorkspaceRoute } from './remoteWorkspaces';

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
