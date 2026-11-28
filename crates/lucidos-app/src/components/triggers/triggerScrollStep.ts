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
