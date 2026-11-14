/** A range in words, `low to high`. User-facing text draws no dash. */
export function rangeLabel(low: string, high: string): string {
  return `${low} to ${high}`;
}
