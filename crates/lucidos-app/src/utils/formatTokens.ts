/** Pretty-print a token count, rounding to the nearest k above 1000. */
export function formatTokens(n: number): string {
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

/** Claude's tokenizer differs ~10-20% from chars/4; callers must label with ≈. */
export function estimateTokens(chars: number): number {
  return Math.round(chars / 4);
}

/** Percent of the context window consumed, clamped 0..100. Returns 0 when
 *  `window` is 0 to avoid NaN; the modal hides the bar in that case. */
export function contextPercent(used: number, window: number): number {
  if (window <= 0) return 0;
  return Math.min(100, Math.round((used / window) * 100));
}
