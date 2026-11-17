import { describe, expect, it, vi } from 'vitest';
import type { ThreadUsersList, ThreadUsers } from './threadUsers';
import { people } from './threadUsers';

const LIST: ThreadUsersList = { items: [], user: { id: 'kim', name: 'Kim' } };

vi.mock('./_fetch', () => ({ request: () => Promise.resolve(LIST) }));

// Pin the SDK type to the engine's wire format
// (crates/lucidos-engine/src/api/thread_users.rs `roles_body`, with
// `gateway_users::UserRef`). The type-check IS the test.
describe('ThreadUsers matches the engine wire format', () => {
  it('names the root, a nullable driver and the watchers', () =>
    ({
      root_thread_id: '00000000-0000-0000-0000-000000000001',
      driver: { id: 'sam', name: 'Sam (A)' },
      watchers: [{ id: 'kim', name: 'Kim' }],
    }) satisfies ThreadUsers);

  it('has no driver on an undriven root thread', () =>
    ({ root_thread_id: 'r', driver: null, watchers: [] }) satisfies ThreadUsers);
});

// Pinned to `engine::thread_users::ThreadUsersSummary` (roles flattened in) and
// the `list` handler's `{ items, user }` body.
describe('ThreadUsersList matches the engine wire format', () => {
  it('lists open root threads with their views and the caller', () =>
    ({
      items: [
        {
          root_thread_id: 'r',
          title: null,
          last_activity: '2026-01-01T00:00:00Z',
          driver: null,
          watchers: [],
          gate_open: true,
          escalated: false,
          needs_team: true,
          yours: false,
        },
      ],
      user: { id: 'kim', name: 'Kim' },
    }) satisfies ThreadUsersList);
});

// An app written before the user rename reads `person` from the alias.
describe('the deprecated threads.people alias', () => {
  it('also names the caller person', async () => {
    const list = await people.list();
    expect(list.person).toEqual({ id: 'kim', name: 'Kim' });
    expect(list.user).toEqual(list.person);
  });
});
