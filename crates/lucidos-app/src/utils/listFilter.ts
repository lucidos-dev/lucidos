export function filterListItems<T extends { label?: string; name?: string; description?: string }>(items: T[], query: string): T[];
export function filterListItems<T extends { label?: string; name?: string; description?: string }>(items: readonly T[], query: string): readonly T[];
export function filterListItems<T extends { label?: string; name?: string; description?: string }>(items: readonly T[], query: string): readonly T[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (terms.length === 0) return items;
  return items.filter((item) => {
    const text = `${item.label ?? item.name ?? ''} ${item.description ?? ''}`.toLowerCase();
    return terms.every((term) => text.includes(term));
  });
}

export function listFilterVisible({ query, coarse, narrow }: { query: string; coarse: boolean; narrow: boolean }): boolean {
  return query.length > 0 || coarse || narrow;
}

