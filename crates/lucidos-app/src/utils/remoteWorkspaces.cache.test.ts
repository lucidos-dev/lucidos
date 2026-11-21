// @vitest-environment jsdom
import { afterEach, expect, it } from 'vitest';
import { mergeRemoteListings, recallRemoteListings, rememberRemoteListings } from './remoteWorkspaces';
afterEach(() => localStorage.clear());
it('restores rows closed while checking and preserves last success after gateway restart', () => {
  const before = [{ gateway: 'cloud', lastSeenAt: '2026-01-01T00:00:00Z', workspaces: [{ id: 'dev', name: 'Dev', health: 'healthy' as const }], unreachable: null }];
  rememberRemoteListings(before);
  expect(recallRemoteListings()[0].unreachable).toBe('Checking gateway');
  expect(mergeRemoteListings(before, [{ gateway: 'cloud', state: 'unreachable', reason: 'down', last_seen_at: null }])[0].lastSeenAt).toBe(before[0].lastSeenAt);
});
it('rejects malformed cache rows and timestamps', () => {
  for (const entry of [null, { gateway: 'cloud', workspaces: [null] }, { gateway: 'cloud', lastSeenAt: {}, workspaces: [] }, { gateway: '../bad', workspaces: [] }]) {
    localStorage.setItem('lucidos-remote-workspaces', JSON.stringify([entry]));
    expect(recallRemoteListings()).toEqual([]);
  }
});
it('does not reuse rows from an earlier pairing with the same gateway name', () => {
  const previous = [{ gateway: 'cloud', pairingId: 'old', host: 'example.test', lastSeenAt: '2026-01-01T00:00:00Z',
    workspaces: [{ id: 'dev', name: 'Dev', health: 'healthy' as const }], unreachable: null }];
  const next = mergeRemoteListings(previous, [{ gateway: 'cloud', pairing_id: 'new', host: 'example.test', state: 'unreachable', reason: 'down', last_seen_at: null }]);
  expect(next[0].workspaces).toEqual([]);
  expect(next[0].lastSeenAt).toBeUndefined();
});
