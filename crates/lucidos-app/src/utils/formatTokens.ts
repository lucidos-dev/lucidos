/** Pretty-print a token count: `M` at a million, `k` above 1000.
 *
 *  A million-token window is the unit the user says out loud ("my default is
 *  1m"), and the model ids carry it as `[1m]`. Rendering it as `1000k` reads as
 *  a different, larger number than the marker it came from.
 *
 *  The promotion is decided on the ROUNDED k, not on `n`, so `1000k` is not a
 *  string this can return. Testing `n` directly leaves the band that rounds up
 *  to 1000k (999,500 and above) below the threshold and printing it. */
export function formatTokens(n: number): string {
  if (n < 1000) return String(n);
  const k = Math.round(n / 1000);
  return k < 1000 ? `${k}k` : `${Math.round(k / 100) / 10}M`;
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
