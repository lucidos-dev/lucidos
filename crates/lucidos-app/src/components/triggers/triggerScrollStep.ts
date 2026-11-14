/** What the Triggers panel should do about a pending trigger deep link, decided
 *  without touching the DOM so the whole table is unit-testable.
 *
 *  `expand` exists because a collapsed section renders none of its members, so
 *  the row's anchor is not there to scroll to. Expanding is a render, not a
 *  landing: the target survives and this is asked again once the row mounts.
 *
 *  `drop` is the consume-once contract. A target naming no trigger is cleared
 *  rather than held, so a stale id can never mark an unrelated row later. */
export type TriggerScrollStep =
  | { kind: 'idle' }
  | { kind: 'drop' }
  | { kind: 'expand'; sectionId: string }
  | { kind: 'scroll'; triggerId: string };

export function resolveTriggerScrollStep(
  target: string | null,
  rows: readonly { id: string; section: string }[],
  collapsedSectionIds: ReadonlySet<string>,
): TriggerScrollStep {
  if (!target) return { kind: 'idle' };
  const row = rows.find((t) => t.id === target);
  if (!row) return { kind: 'drop' };
  if (collapsedSectionIds.has(row.section)) return { kind: 'expand', sectionId: row.section };
  return { kind: 'scroll', triggerId: target };
}
