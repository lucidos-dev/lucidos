/** What the Triggers panel should do about a pending trigger deep link, decided
 *  without touching the DOM so the whole table is unit-testable.
 *
 *  `expand` exists because a collapsed group renders none of its members, so
 *  the row's anchor is not there to scroll to. Expanding is a render, not a
 *  landing: the target survives and this is asked again once the row mounts.
 *
 *  `drop` is the consume-once contract. A target naming no trigger is cleared
 *  rather than held, so a stale id can never mark an unrelated row later. */
export type TriggerScrollStep =
  | { kind: 'idle' }
  | { kind: 'drop' }
  | { kind: 'expand'; groupId: string }
  | { kind: 'scroll'; triggerId: string };

export function resolveTriggerScrollStep(
  target: string | null,
  rows: readonly { id: string; group_id?: string }[],
  collapsedGroupIds: ReadonlySet<string>,
): TriggerScrollStep {
  if (!target) return { kind: 'idle' };
  const row = rows.find((t) => t.id === target);
  if (!row) return { kind: 'drop' };
  const groupId = row.group_id;
  if (groupId && collapsedGroupIds.has(groupId)) return { kind: 'expand', groupId };
  return { kind: 'scroll', triggerId: target };
}
