import { describe, it } from 'vitest';
import type { ThreadPeopleList, ThreadPeople } from './threadPeople';

// Pin the SDK type to the engine's wire format
// (crates/lucidos-engine/src/api/thread_people.rs `roles_body`, with
// `gateway_operators::OperatorRef`). The type-check IS the test.
describe('ThreadPeople matches the engine wire format', () => {
  it('names the root, a nullable driver and the watchers', () =>
    ({
      root_thread_id: '00000000-0000-0000-0000-000000000001',
      driver: { id: 'sam', name: 'Sam via A' },
      watchers: [{ id: 'kim', name: 'Kim' }],
    }) satisfies ThreadPeople);

  it('has no driver on an undriven root thread', () =>
    ({ root_thread_id: 'r', driver: null, watchers: [] }) satisfies ThreadPeople);
});

// Pinned to `engine::thread_people::ThreadPeopleSummary` (roles flattened in) and
// the `list` handler's `{ items, person }` body.
describe('ThreadPeopleList matches the engine wire format', () => {
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
      person: { id: 'kim', name: 'Kim' },
    }) satisfies ThreadPeopleList);
});
