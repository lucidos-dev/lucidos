import { describe, it, expect } from 'vitest';
import { sectionTokenScale } from '../sectionTokens';
import { groupSections } from '../contextGrouping';
import type { ContextCapture, ContextSection } from '../../../store/types';


function section(name: string, chars: number, role: ContextSection['role'] = 'user'): ContextSection {
  return { name, budget_delta_chars: chars, content_chars: chars, role };
}

function capture(over: Partial<ContextCapture> = {}): ContextCapture {
  return {
    producer: 'main_llm',
    model: 'claude-opus-5[1m]',
    context_window: 1_000_000,
    sections: [
      section('System Instructions', 147_800, 'system'),
      section('Prior turn', 17_400, 'prior_message'),
      section('Conversation', 374_900, 'user'),
    ],
    tools: [],
    estimated_total_tokens: 360_066,
    trimmed: false,
    ...over,
  };
}

function renderedRoleTotals(snap: ContextCapture): number {
  const tokens = sectionTokenScale(snap);
  return groupSections(snap.sections)
    .map(role => role.innerGroups.flatMap(ig => ig.sections).reduce((a, s) => a + s.budget_delta_chars, 0))
    .reduce((a, chars) => a + tokens(chars), 0);
}

describe('sectionTokenScale', () => {
  it('sums the tree to the measured header when the provider reported usage', () => {
    const snap = capture({
      usage: { input_tokens: 205_000, output_tokens: 196, cache_read_tokens: 203_000, cache_creation_tokens: 2_000 },
    });
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
    expect(measured).toBeLessThan(estimated);
    expect(measured / estimated).toBeCloseTo(205_000 / 360_066, 3);
  });

  it('splits proportionally, so a section twice the size reads twice the tokens', () => {
    const tokens = sectionTokenScale(capture());
    expect(tokens(2_000)).toBeCloseTo(tokens(1_000) * 2, -0.5);
  });

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
    const tokens = sectionTokenScale(capture({ estimated_total_tokens: 0, legacy: true }));
    expect(tokens(147_800)).toBe(0);
  });

  it('returns zero for a stripped snapshot before its lazy fetch resolves', () => {
    const tokens = sectionTokenScale(
      capture({ sections: [], sections_stripped: true, event_id: 'e1' }),
    );
    expect(tokens(147_800)).toBe(0);
  });

  it('divides by the budget delta, so a delta section does not break the sum', () => {
    const snap = capture({
      sections: [
        section('System Instructions', 147_800, 'system'),
        section('Prior turn', 17_400, 'prior_message'),
        { name: 'Conversation', budget_delta_chars: 374_900, content_chars: 540_100, role: 'user' },
      ],
      usage: { input_tokens: 205_000, output_tokens: 196, cache_read_tokens: 203_000, cache_creation_tokens: 2_000 },
    });
    expect(renderedRoleTotals(snap)).toBeCloseTo(205_000, -1);
  });

  it('renders a pre-split row, which measured no region', () => {
    const snap = capture({
      sections: [{ name: 'Conversation', budget_delta_chars: 500, role: 'user' }],
      usage: { input_tokens: 1_000, output_tokens: 5, cache_read_tokens: 0, cache_creation_tokens: 0 },
    });
    expect(renderedRoleTotals(snap)).toBeCloseTo(1_000, -1);
  });
});
