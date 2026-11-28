import { includeDeletedFilterOptions } from './store';

export type DeletableOption = {
  id: string;
  label: string;
  deleted: boolean;
  lastActivity?: string;
};

export function visibleFilterOptions<T extends DeletableOption>(
  all: readonly T[],
  selected: Set<string>,
): T[] {
  const includeDeleted = includeDeletedFilterOptions.value;
  return all
    .filter(o => includeDeleted || !o.deleted || selected.has(o.id))
    .sort((a, b) => {
      if (a.deleted !== b.deleted) return a.deleted ? 1 : -1;
      if (a.deleted) {
        const aTime = a.lastActivity ?? '';
        const bTime = b.lastActivity ?? '';
        if (aTime !== bTime) return bTime.localeCompare(aTime);
      }
      return a.label.localeCompare(b.label);
    });
}
