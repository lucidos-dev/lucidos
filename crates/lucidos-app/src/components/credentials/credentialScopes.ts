
export function initialScopeRows(
  requested: string[] | undefined,
  stored: string[] | undefined,
): string[] {
  const declared = requested?.length ? requested : stored;
  const rows = (declared ?? []).filter((u) => u.trim() !== '');
  return rows.length > 0 ? rows : [''];
}

export function setScopeRow(rows: string[], index: number, value: string): string[] {
  return rows.map((row, i) => (i === index ? value : row));
}

export function addScopeRow(rows: string[]): string[] {
  return [...rows, ''];
}

export function removeScopeRow(rows: string[], index: number): string[] {
  const kept = rows.filter((_, i) => i !== index);
  return kept.length > 0 ? kept : [''];
}

export function submittedScopes(rows: string[]): string[] {
  const out: string[] = [];
  for (const row of rows) {
    const trimmed = row.trim();
    if (trimmed !== '' && !out.includes(trimmed)) out.push(trimmed);
  }
  return out;
}
