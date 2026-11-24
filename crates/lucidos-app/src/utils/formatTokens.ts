export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  const k = Math.round(n / 1000);
  return k < 1000 ? `${k}k` : `${Math.round(k / 100) / 10}M`;
}


export function formatContextWindow(tokens: number | null): string {
  if (tokens === null) return 'context window: inferred';
  return `context window: ${formatTokens(tokens)}`;
}

export function contextPercent(used: number, window: number): number {
  if (window <= 0) return 0;
  return Math.round((used / window) * 100);
}
