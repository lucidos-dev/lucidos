import { describe, it } from 'vitest';
import type { WorkItemList, WorkItemRoles } from './workItems';

// Pin the SDK type to the engine's wire format
// (crates/lucidos-engine/src/api/work_items.rs `roles_body`, with
// `gateway_operators::OperatorRef`). The type-check IS the test.
describe('WorkItemRoles matches the engine wire format', () => {
  it('names the root, a nullable driver and the watchers', () =>
    ({
      root_thread_id: '00000000-0000-0000-0000-000000000001',
      driver: { id: 'sam', name: 'Sam via A' },
      watchers: [{ id: 'kim', name: 'Kim' }],
    }) satisfies WorkItemRoles);

  it('has no driver on an undriven work item', () =>
    ({ root_thread_id: 'r', driver: null, watchers: [] }) satisfies WorkItemRoles);
});

// Pinned to `engine::work_item::WorkItemSummary` (roles flattened in) and
// the `list` handler's `{ items, person }` body.
describe('WorkItemList matches the engine wire format', () => {
  it('lists open work items with their views and the caller', () =>
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
    }) satisfies WorkItemList);
});
