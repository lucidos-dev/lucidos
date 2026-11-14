import router from '../../../../lucidos-engine/src/api/users.rs?raw';
import { expect, it, vi } from 'vitest';
import { listUsers } from './users';

const json = vi.hoisted(() => vi.fn());

vi.mock('./_core', () => ({
  API: '/notes/api/v1',
  json,
  mutatingFetch: vi.fn(),
  throwIfNotOk: vi.fn(),
}));

it('reads the user listing from the route the engine registers', async () => {
  const route = router.match(/\.route\("([^"]+)", get\(users\)\)/)?.[1];
  expect(route).toBeDefined();
  const listing = { users: [], you: null, team: false };
  json.mockResolvedValue(listing);

  expect(await listUsers()).toEqual(listing);
  expect(json).toHaveBeenCalledWith(`/notes/api/v1${route}`);
});
