import { afterEach, describe, expect, it, vi } from 'vitest';
import type { UsersResponse } from '../../api/client';

const listUsers = vi.hoisted(() => vi.fn<() => Promise<UsersResponse>>());
vi.mock('../../api/client', async (importOriginal) => ({
  ...await importOriginal<typeof import('../../api/client')>(),
  listUsers,
}));

import { loadUsers, users } from './users';

const listing = (you: string): UsersResponse => ({ users: [{ id: you, name: you, has_device: true }], you, team: false });

function held() {
  let land!: (body: UsersResponse) => void;
  let refuse!: (error: unknown) => void;
  listUsers.mockImplementationOnce(() => new Promise((resolve, reject) => { land = resolve; refuse = reject; }));
  return { land: (body: UsersResponse) => land(body), refuse: (error: unknown) => refuse(error) };
}

afterEach(() => {
  users.value = { status: 'not-loaded' };
  listUsers.mockReset();
});

describe('two overlapping user reads', () => {
  it('keep the newer list when the older answer lands last', async () => {
    const older = held();
    const newer = held();
    const first = loadUsers();
    const second = loadUsers();
    newer.land(listing('new'));
    await second;
    older.land(listing('old'));
    await first;
    expect(users.value).toEqual({ status: 'loaded', data: listing('new') });
  });

  it('never flip the list to failed when the older read fails last', async () => {
    const older = held();
    const newer = held();
    const first = loadUsers();
    const second = loadUsers();
    newer.land(listing('new'));
    await second;
    older.refuse(new Error('down'));
    await first;
    expect(users.value).toEqual({ status: 'loaded', data: listing('new') });
  });

  it('show the older list when it lands first, then the newer one', async () => {
    const older = held();
    const newer = held();
    const first = loadUsers();
    const second = loadUsers();
    older.land(listing('old'));
    await first;
    expect(users.value).toEqual({ status: 'loaded', data: listing('old') });
    newer.land(listing('new'));
    await second;
    expect(users.value).toEqual({ status: 'loaded', data: listing('new') });
  });
});
