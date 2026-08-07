/** Pretty-print a token count, rounding to the nearest k above 1000. */
export function formatTokens(n: number): string {
  if (n >= 1000) return `${Math.round(n / 1000)}k`;
  return String(n);
}

// There is deliberately NO chars-to-tokens helper here. `estimateTokens`
// (`chars × 2/3`) lived at this spot and was a hand copy of the engine's
// *trim-budget* ratio of 1.5 chars/token, which is conservative on purpose so
// the packer cannot overflow the context window. Displaying it made the LLM
// Context Viewer report a measured 205k prompt as 361k, contradicting the
// `usage.input_tokens` header directly above the tree. The viewer now scales
// each section against the capture's own headline total instead
// (`components/chat/sectionTokens.ts`), which both fixes the number and means
// the ratio is no longer duplicated across two languages with nothing keeping
// them in step. Do not reintroduce one here.

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
