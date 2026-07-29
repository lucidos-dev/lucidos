/** Pretty-print a token count, rounding to the nearest k above 1000. */
export function formatTokens(n: number): string {
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

/**
 * Conservative chars-to-tokens estimate matching the engine's trim budget
 * (1.5 chars/token — see `agent_context_char_budget` in
 * `crates/lucidos-engine/src/engine/context.rs`).
 *
 * The old prose-shaped `chars / 4` undercounted JSON-heavy tool I/O ~2.4×
 * — the modal would show "200 K tokens" for a request the API then
 * rejected at 1.5 M. Callers must still label with ≈ (Claude's tokenizer
 * varies a bit per content type).
 */
export function estimateTokens(chars: number): number {
  return Math.round((chars * 2) / 3);
}

/** Render a model's declared context window for the Settings → Models row.
 *  `null` means the engine infers it from the id — worth saying out loud,
 *  because that fallback only knows Claude and GPT-5 ids and silently treats
 *  everything else as 200k. */
export function formatContextWindow(tokens: number | null): string {
  if (tokens === null) return 'context window: inferred';
  return `context window: ${formatTokens(tokens)}`;
}

/** Percent of the context window consumed, clamped 0..100. Returns 0 when
 *  `window` is 0 to avoid NaN; the modal hides the bar in that case. */
export function contextPercent(used: number, window: number): number {
  if (window <= 0) return 0;
  return Math.min(100, Math.round((used / window) * 100));
}
