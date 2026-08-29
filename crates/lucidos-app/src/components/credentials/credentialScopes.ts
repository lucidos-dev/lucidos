/** The *credential scope* editor's list logic, kept pure so it is testable
 *  without mounting the modal.
 *
 *  A credential is presented only to a base URL it declares, and a provider
 *  often needs several: one Binance key pair signs `api.binance.com` and
 *  `fapi.binance.com`. The form therefore edits a LIST, and these helpers own
 *  every way that list changes. */

/** The rows the form starts with.
 *
 *  Always at least one, so a fresh credential shows a field to type into
 *  rather than a lone Add button. */
export function seedScopeRows(stored: string[] | undefined): string[] {
  const rows = (stored ?? []).filter((u) => u.trim() !== '');
  return rows.length > 0 ? rows : [''];
}

export function setScopeRow(rows: string[], index: number, value: string): string[] {
  return rows.map((row, i) => (i === index ? value : row));
}

export function addScopeRow(rows: string[]): string[] {
  return [...rows, ''];
}

/** Remove one row, keeping at least one field on screen.
 *
 *  Removing the last row clears it instead of emptying the list. An empty scope
 *  is a real state (it means the credential goes nowhere), and `submittedScopes`
 *  is what expresses it: a single blank field submits as no scope. */
export function removeScopeRow(rows: string[], index: number): string[] {
  const kept = rows.filter((_, i) => i !== index);
  return kept.length > 0 ? kept : [''];
}

/** What the form sends: trimmed, blanks dropped, duplicates collapsed.
 *
 *  Mirrors the engine's `normalized_base_urls`, which is the authority and
 *  re-runs this on every write. Doing it here as well keeps the request free of
 *  the empty row the editor always carries. */
export function submittedScopes(rows: string[]): string[] {
  const out: string[] = [];
  for (const row of rows) {
    const trimmed = row.trim();
    if (trimmed !== '' && !out.includes(trimmed)) out.push(trimmed);
  }
  return out;
}
