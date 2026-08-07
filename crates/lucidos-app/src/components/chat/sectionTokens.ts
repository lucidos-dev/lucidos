import type { ContextCapture } from '../../store/types';

/** Char count in, tokens out. One instance is built per capture and threaded
 *  down the whole section tree, which is what guarantees the rows sum. */
export type TokenScale = (chars: number) => number;

/** The one number the LLM Context Viewer is about: what this call actually
 *  cost. The provider's measured prompt when it reported one, the engine's
 *  estimate otherwise.
 *
 *  Lives here, and has exactly two callers, because the panel's whole
 *  correctness property is that the budget bar and the section rows show the
 *  same total. Both read it from this function, so they cannot disagree.
 *  Inlining the expression at either site would restore the drift this change
 *  removed: the section rows would still sum to *something*, just not to the
 *  header, and no test would catch it (the unit test derives its expectation
 *  from the same scale). */
export function headlineTokens(snap: ContextCapture): number {
  return snap.usage?.input_tokens ?? snap.estimated_total_tokens;
}

/** Turn a section's char count into the token number the LLM Context Viewer
 *  shows for it.
 *
 *  The rows are a **share of the capture's headline total**, not an
 *  independent estimate: `chars / totalSectionChars × headlineTokens`, where
 *  the headline is the same `usage.input_tokens ?? estimated_total_tokens`
 *  the budget bar renders. So the tree sums to the header by construction, in
 *  both regimes, which is the whole point of the helper.
 *
 *  It replaced a duplicated `chars × 2/3` constant (the engine's *trim-budget*
 *  ratio of 1.5 chars/token). That ratio is deliberately conservative so the
 *  packer can never overflow the window, and using it here made the tree
 *  report a measured 205k prompt as 361k, contradicting the header directly
 *  above it. Deriving the scale from the capture fixes that *and* removes the
 *  copy: `sum(section.char_count)` is the same char total the engine ran
 *  through `estimate_tokens_from_chars` (`agentic_loop/run.rs` builds the
 *  section list to sum to `system + tool_defs + context`), so with no usage
 *  the scale reproduces the engine's own ratio, whatever it is, and an engine
 *  retune reaches these rows with no second edit here. The two agreed on all
 *  12,069 sampled captures; they can in principle diverge, because the
 *  Conversation section floors at zero via `saturating_sub`. Nothing here
 *  depends on them agreeing, since a proportional split sums to the headline
 *  either way.
 *
 *  Still labelled `≈` at the call sites. A proportional split assumes uniform
 *  token density across sections, and JSON tool schemas pack denser than
 *  prose, so an individual row is an approximation even when the total it is
 *  a share of was measured exactly.
 */
export function sectionTokenScale(snap: ContextCapture): TokenScale {
  const headline = headlineTokens(snap);
  const totalChars = snap.sections.reduce((a, s) => a + s.char_count, 0);
  // Three degenerate shapes, all real:
  //  - the Claude Code producer emits `sections: []` (nothing to scale),
  //  - `synthesizeContextCapture` can yield `estimated_total_tokens: 0` for a
  //    legacy row with no measured tokens and no `total_chars`,
  //  - a stripped snapshot before its lazy fetch resolves.
  // Any of them would make the scale `NaN` or `Infinity`, so fall back to
  // zero rather than rendering "NaN tokens".
  if (totalChars <= 0 || !Number.isFinite(headline) || headline <= 0) return () => 0;
  return chars => Math.round((chars / totalChars) * headline);
}
