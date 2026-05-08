import { describe, it, expect } from 'vitest';
import { renderDiffMarked, additionRuns } from './RenderedDiff';
import type { DiffFile } from '../../store/store';

function additionAt(start: number, count: number): DiffFile {
  return {
    path: 'x.md',
    status: 'modified',
    hunks: [{
      old_start: start,
      old_count: 0,
      new_start: start,
      new_count: count,
      lines: Array.from({ length: count }, () => ({ type: 'addition' as const, content: 'x' })),
    }],
  };
}

describe('renderDiffMarked line tracking', () => {
  it('marks the right list item when additions land on later lines', () => {
    // 14 lines total; lines 11-14 are the new last items.
    // Realistic shape: headings + paragraphs with blank-line separators
    // (whose trailing \n marked emits as separate `space` tokens).
    const content = `# Title

Some text.

## Section

Para one.

Para two.

1. Item one
2. Item two
3. Item three
4. Item four
`;
    // additions at lines 11-14 → all four items added
    const runs = additionRuns(additionAt(11, 4));
    const html = renderDiffMarked(content, runs);

    // The list block (or every item) should be marked added.
    // Earlier headings/paragraphs must NOT be marked added.
    expect(html).toContain('diff-rendered-block-added');
    expect(html).not.toMatch(/<h1[^>]*class="[^"]*diff-rendered-(added|changed)/);
    expect(html).not.toMatch(/<h2[^>]*class="[^"]*diff-rendered-(added|changed)/);
    expect(html).not.toMatch(/<p[^>]*class="[^"]*diff-rendered-(added|changed)/);
    // The pre-list paragraphs should NOT be wrapped in a status div.
    expect(html).not.toMatch(/<div class="diff-rendered-(added|changed)"><h/);
    expect(html).not.toMatch(/<div class="diff-rendered-(added|changed)"><p>(Some text|Para one|Para two)/);
  });

  it('marks only the new list item, not earlier items', () => {
    const content = `Intro.

1. First
2. Second
3. Third
4. Fourth (new)
`;
    // additions at line 6 only (the "Fourth (new)" line)
    const runs = additionRuns(additionAt(6, 1));
    const html = renderDiffMarked(content, runs);

    // Item 4 should be marked added; items 1-3 should not.
    // Look for the marked class on the <li> for "Fourth"
    expect(html).toMatch(/<li[^>]*class="[^"]*diff-rendered-added[^"]*"[^>]*>Fourth/);
    expect(html).not.toMatch(/<li[^>]*class="[^"]*diff-rendered-added[^"]*"[^>]*>First/);
    expect(html).not.toMatch(/<li[^>]*class="[^"]*diff-rendered-added[^"]*"[^>]*>Second/);
    expect(html).not.toMatch(/<li[^>]*class="[^"]*diff-rendered-added[^"]*"[^>]*>Third/);
  });

  it('does not mark anything when additions fall outside content', () => {
    const content = `Para one.

Para two.
`;
    const runs = additionRuns(additionAt(99, 1));
    const html = renderDiffMarked(content, runs);
    expect(html).not.toContain('diff-rendered-added');
    expect(html).not.toContain('diff-rendered-changed');
  });
});
