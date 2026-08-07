import { describe, it, expect } from 'vitest';
import { sectionTokenScale } from '../sectionTokens';
import { groupSections } from '../contextGrouping';
import type { ContextCapture, ContextSection } from '../../../store/types';

/** The LLM Context Viewer rendered its section rows through a hand-copied
 *  `chars × 2/3` (the engine's *trim-budget* ratio of 1.5 chars/token), while
 *  the header rendered the provider's measured `usage.input_tokens`. On the
 *  capture that surfaced this the tree summed to 361k under a header reading
 *  205k. These pin the property that replaced it: the rows are shares of the
 *  headline, so the tree sums to the header by construction. */

function section(name: string, chars: number, role: ContextSection['role'] = 'user'): ContextSection {
  return { name, char_count: chars, role };
}

/** The reported capture, to scale: 540.1K chars of sections against a real
 *  205k-token prompt. Section sizes are the three role totals from it. */
function capture(over: Partial<ContextCapture> = {}): ContextCapture {
  return {
    producer: 'main_llm',
    model: 'claude-opus-5@default[1m]',
    context_window: 1_000_000,
    sections: [
      section('System Instructions', 147_800, 'system'),
      section('Prior turn', 17_400, 'prior_message'),
      section('Conversation', 374_900, 'user'),
    ],
    tools: [],
    estimated_total_tokens: 360_066, // 540,100 × 2/3, what the engine stored
    trimmed: false,
    ...over,
  };
}

/** Sum of what the tree actually renders: every role header, which is what a
 *  reader adds up. Mirrors `ContextCapturePanel`'s grouping so a change to the
 *  role buckets cannot quietly break the sum. */
function renderedRoleTotals(snap: ContextCapture): number {
  const tokens = sectionTokenScale(snap);
  return groupSections(snap.sections)
    .map(role => role.innerGroups.flatMap(ig => ig.sections).reduce((a, s) => a + s.char_count, 0))
    .reduce((a, chars) => a + tokens(chars), 0);
}

describe('sectionTokenScale', () => {
  it('sums the tree to the measured header when the provider reported usage', () => {
    const snap = capture({
      usage: { input_tokens: 205_000, output_tokens: 196, cache_read_tokens: 203_000, cache_creation_tokens: 2_000 },
    });
    // Rounding can move each of the three rows by at most half a token.
    expect(renderedRoleTotals(snap)).toBeCloseTo(205_000, -1);
  });

  it('sums the tree to the estimate when the provider reported none', () => {
    const snap = capture();
    expect(renderedRoleTotals(snap)).toBeCloseTo(360_066, -1);
  });

  it('prefers measured usage over the estimate, so the tree tracks the header', () => {
    const estimated = sectionTokenScale(capture())(147_800);
    const measured = sectionTokenScale(
      capture({
        usage: { input_tokens: 205_000, output_tokens: 0, cache_read_tokens: 0, cache_creation_tokens: 0 },
      }),
    )(147_800);
    // 205k/360k of the estimate: the correction the whole change exists for.
    expect(measured).toBeLessThan(estimated);
    expect(measured / estimated).toBeCloseTo(205_000 / 360_066, 3);
  });

  it('splits proportionally, so a section twice the size reads twice the tokens', () => {
    const tokens = sectionTokenScale(capture());
    // Within a token: each row is rounded independently, so doubling the chars
    // can land half a token either side of doubling the rounded result.
    expect(tokens(2_000)).toBeCloseTo(tokens(1_000) * 2, -0.5);
  });

  // INV-6: three degenerate shapes that all occur in the wild. None may render
  // `NaN tokens` or throw.
  it('returns zero for a capture with no sections (the Claude Code producer)', () => {
    const tokens = sectionTokenScale(
      capture({
        producer: 'claude_code',
        sections: [],
        usage: { input_tokens: 134_449, output_tokens: 16, cache_read_tokens: 133_767, cache_creation_tokens: 681 },
      }),
    );
    expect(tokens(0)).toBe(0);
    expect(Number.isFinite(tokens(1_000))).toBe(true);
  });

  it('returns zero for a legacy synthesis with no headline total', () => {
    // `synthesizeContextCapture` falls back to 0 when the legacy row carried
    // neither a measured token count nor a char total.
    const tokens = sectionTokenScale(capture({ estimated_total_tokens: 0, legacy: true }));
    expect(tokens(147_800)).toBe(0);
  });

  it('returns zero for a stripped snapshot before its lazy fetch resolves', () => {
    const tokens = sectionTokenScale(
      capture({ sections: [], sections_stripped: true, event_id: 'e1' }),
    );
    expect(tokens(147_800)).toBe(0);
  });
});
