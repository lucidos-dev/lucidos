import { describe, expect, it } from 'vitest';
import { keepUndrawnKeys, moveOrderKey, orderWorkspaces, stepOrderKey } from './workspaceOrder';
import type { RemoteGatewayView } from './remoteWorkspaces';
import type { ListedWorkspace } from '../api/client/control';

const local = (id: string) => ({ id, name: id });
const listed = (id: string): ListedWorkspace => ({ id, name: id, health: 'healthy' });
const gateway = (name: string, ...ids: string[]): RemoteGatewayView => ({
  gateway: name,
  workspaces: ids.map(listed),
  unreachable: null,
});

const keys = (entries: ReturnType<typeof orderWorkspaces>) => entries.map((e) => e.key);

describe('orderWorkspaces', () => {
  const locals = [local('dev'), local('notes'), local('home')];
  const remotes = [gateway('cloud', 'web', 'api'), gateway('lab', 'scratch')];

  it('draws local rows in registry order, then remote rows by gateway, with no stored order', () => {
    expect(keys(orderWorkspaces(locals, remotes, []))).toEqual(['dev', 'notes', 'home', 'web@cloud', 'api@cloud', 'scratch@lab']);
  });

  it('puts ordered keys first and the rest after by the default rule', () => {
    const entries = orderWorkspaces(locals, remotes, ['api@cloud', 'home']);
    expect(keys(entries)).toEqual(['api@cloud', 'home', 'dev', 'notes', 'web@cloud', 'scratch@lab']);
  });

  it('tells local rows from remote rows and carries the gateway view', () => {
    const [first, second] = orderWorkspaces(locals, remotes, ['web@cloud', 'dev']);
    expect(first).toMatchObject({ kind: 'remote', key: 'web@cloud', workspace: { id: 'web' }, view: { gateway: 'cloud' } });
    expect(second).toMatchObject({ kind: 'local', key: 'dev', workspace: { id: 'dev' } });
  });

  it('shows every workspace exactly once, whatever the stored order holds', () => {
    const order = ['gone', 'home', 'home', 'nope@cloud', 'web@cloud', 'x@missing-gateway', 'web@cloud'];
    const drawn = keys(orderWorkspaces(locals, remotes, order));
    expect(drawn).toEqual(['home', 'web@cloud', 'dev', 'notes', 'api@cloud', 'scratch@lab']);
    expect(new Set(drawn).size).toBe(drawn.length);
  });

  it('keeps a remote slug apart from a local workspace with the same slug', () => {
    expect(keys(orderWorkspaces([local('web')], [gateway('cloud', 'web')], ['web@cloud']))).toEqual(['web@cloud', 'web']);
  });
});

describe('moveOrderKey', () => {
  const drawn = ['a', 'b', 'c', 'd@cloud'];

  it('moves a key to an index in the drawn list', () => {
    expect(moveOrderKey(drawn, 'd@cloud', 0)).toEqual(['d@cloud', 'a', 'b', 'c']);
    expect(moveOrderKey(drawn, 'a', 2)).toEqual(['b', 'c', 'a', 'd@cloud']);
  });

  it('clamps the index to the list', () => {
    expect(moveOrderKey(drawn, 'b', 99)).toEqual(['a', 'c', 'd@cloud', 'b']);
    expect(moveOrderKey(drawn, 'c', -4)).toEqual(['c', 'a', 'b', 'd@cloud']);
  });

  it('returns the list unchanged for a key it does not hold', () => {
    expect(moveOrderKey(drawn, 'zz', 0)).toEqual(drawn);
  });
});

describe('stepOrderKey', () => {
  const drawn = ['a', 'b', 'c'];

  it('moves a key one place up or down', () => {
    expect(stepOrderKey(drawn, 'b', 'up')).toEqual(['b', 'a', 'c']);
    expect(stepOrderKey(drawn, 'b', 'down')).toEqual(['a', 'c', 'b']);
  });

  it('leaves the ends where they are', () => {
    expect(stepOrderKey(drawn, 'a', 'up')).toEqual(drawn);
    expect(stepOrderKey(drawn, 'c', 'down')).toEqual(drawn);
  });
});

describe('keepUndrawnKeys', () => {
  it('keeps a stored key that is not drawn in its place', () => {
    expect(keepUndrawnKeys(['home@cloud', 'dev', 'notes'], ['notes', 'dev'])).toEqual(['home@cloud', 'notes', 'dev']);
  });

  it('puts drawn keys the store never held after the stored ones', () => {
    expect(keepUndrawnKeys(['gone', 'a'], ['b', 'a'])).toEqual(['gone', 'b', 'a']);
    expect(keepUndrawnKeys([], ['c', 'a', 'b'])).toEqual(['c', 'a', 'b']);
  });

  it('drops a repeated stored key', () => {
    expect(keepUndrawnKeys(['a', 'x', 'a'], ['b', 'a'])).toEqual(['b', 'x', 'a']);
  });
});
