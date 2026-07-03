import { describe, it, expect } from 'vitest';
import type { ContextCapture, ContextSection } from '../../store/types';
import { mergeContextCaptureSections, needsLazyFetch } from './loadStrippedSections';

function baseSnap(over: Partial<ContextCapture> = {}): ContextCapture {
  return {
    producer: 'main_llm',
    model: 'claude-opus-4-7',
    context_window: 200_000,
    sections: [],
    tools: [],
    estimated_total_tokens: 1234,
    trimmed: false,
    ...over,
  };
}

describe('needsLazyFetch', () => {
  it('returns true when sections_stripped and event_id are set (snapshot path)', () => {
    expect(needsLazyFetch(baseSnap({ sections_stripped: true, event_id: 'eid' }))).toBe(true);
  });

  it('returns false on live SSE captures (server did not strip)', () => {
    expect(needsLazyFetch(baseSnap({ sections_stripped: false, event_id: 'eid' }))).toBe(false);
    expect(needsLazyFetch(baseSnap({ event_id: 'eid' }))).toBe(false);
  });

  it('returns false on legacy syntheses (no source event id to fetch)', () => {
    expect(needsLazyFetch(baseSnap({ sections_stripped: true, legacy: true }))).toBe(false);
    expect(needsLazyFetch(baseSnap({ sections_stripped: true }))).toBe(false);
  });
});

describe('mergeContextCaptureSections', () => {
  it('attaches fetched sections + tools and clears the stripped flag', () => {
    const stripped = baseSnap({ sections_stripped: true, event_id: 'eid' });
    const sections: ContextSection[] = [
      { name: 'system', char_count: 100, role: 'system' },
      { name: 'history', char_count: 50, role: 'prior_message' },
    ];
    const merged = mergeContextCaptureSections(stripped, { sections, tools: ['edit', 'search'] });
    expect(merged.sections).toEqual(sections);
    expect(merged.tools).toEqual(['edit', 'search']);
    expect(merged.sections_stripped).toBe(false);
    // Inline-chip fields preserved verbatim.
    expect(merged.producer).toBe('main_llm');
    expect(merged.model).toBe('claude-opus-4-7');
    expect(merged.context_window).toBe(200_000);
    expect(merged.estimated_total_tokens).toBe(1234);
  });

  it('does not mutate the input snapshot', () => {
    const stripped = baseSnap({ sections_stripped: true, event_id: 'eid' });
    const original = JSON.parse(JSON.stringify(stripped));
    mergeContextCaptureSections(stripped, { sections: [{ name: 'x', char_count: 1 }], tools: [] });
    expect(stripped).toEqual(original);
  });
});
