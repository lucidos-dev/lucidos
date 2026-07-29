import { describe, it, expect } from 'vitest';
import { renderDiffMarked, additionRuns, deletionRuns, hunkCoverage } from './RenderedDiff';
import type { DiffFile, DiffLine } from '../../store/store';

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

type LineSpec = ['c' | '+' | '-', string];

function fileFromLines(newStart: number, lines: LineSpec[]): DiffFile {
  const diffLines: DiffLine[] = lines.map(([t, content]) => ({
    type: t === '+' ? 'addition' : t === '-' ? 'deletion' : 'context',
    content,
  }));
  return {
    path: 'x.md',
    status: 'modified',
    hunks: [{
      old_start: 1,
      old_count: diffLines.filter(l => l.type !== 'addition').length,
      new_start: newStart,
      new_count: diffLines.filter(l => l.type !== 'deletion').length,
      lines: diffLines,
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

describe('deletionRuns', () => {
  it('anchors a removed run to the new-file line it precedes', () => {
    const file = fileFromLines(1, [
      ['c', '# Title'],
      ['c', ''],
      ['c', 'Para A.'],
      ['c', ''],
      ['-', 'Old para.'],
      ['-', ''],
      ['c', 'Para B.'],
    ]);
    expect(deletionRuns(file)).toEqual([{ anchor: 5, lines: ['Old para.', ''] }]);
  });

  it('groups consecutive deletions and splits separate runs', () => {
    const file = fileFromLines(1, [
      ['-', 'gone top'],   // before new line 1
      ['c', 'keep'],       // new line 1
      ['+', 'new'],        // new line 2
      ['-', 'gone mid'],   // before new line 3
      ['c', 'tail'],       // new line 3
    ]);
    expect(deletionRuns(file)).toEqual([
      { anchor: 1, lines: ['gone top'] },
      { anchor: 3, lines: ['gone mid'] },
    ]);
  });
});

describe('hunkCoverage', () => {
  it('returns each hunk new-file extent and skips zero-new-count hunks', () => {
    const file: DiffFile = {
      path: 'x.md',
      status: 'modified',
      hunks: [
        { old_start: 1, old_count: 3, new_start: 5, new_count: 5, lines: [] },
        { old_start: 20, old_count: 1, new_start: 22, new_count: 0, lines: [] },
      ],
    };
    expect(hunkCoverage(file)).toEqual([{ start: 5, end: 9 }]);
  });
});

describe('renderDiffMarked coverage scoping', () => {
  // Single-line blocks separated by single blank lines, so line N is literally
  // the Nth line (matches the line-tracking the other tests rely on).
  const content = `# Title

Intro para.

## Section A

Changed para.

## Section B

Unchanged tail.
`;

  it('renders the whole document when no coverage is given', () => {
    const html = renderDiffMarked(content, [], []);
    expect(html).toContain('Title');
    expect(html).toContain('Intro para.');
    expect(html).toContain('Unchanged tail.');
    expect(html).not.toContain('diff-rendered-gap');
  });

  it('omits blocks outside the hunk coverage and collapses them into gaps', () => {
    // Cover only lines 5-9 (## Section A … ## Section B).
    const html = renderDiffMarked(content, [], [], [{ start: 5, end: 9 }]);
    expect(html).toContain('Section A');
    expect(html).toContain('Changed para.');
    expect(html).toContain('Section B');
    // Leading (Title, Intro) and trailing (tail) content is omitted.
    expect(html).not.toContain('Title');
    expect(html).not.toContain('Intro para.');
    expect(html).not.toContain('Unchanged tail.');
    // One leading gap + one trailing gap.
    expect(html.match(/diff-rendered-gap/g)).toHaveLength(2);
  });

  it('keeps an inline deletion when scoped, with the leading gap above it', () => {
    // A deletion anchored at line 7 (before "Changed para."), inside coverage.
    const dels = [{ anchor: 7, lines: ['Removed old line.'] }];
    const html = renderDiffMarked(content, [], dels, [{ start: 5, end: 9 }]);
    expect(html).toContain('diff-rendered-removed');
    expect(html).toContain('Removed old line.');
    expect(html).toContain('Section A');
    expect(html).toContain('Changed para.');
    // The omitted leading content (Title, Intro) collapses to one gap above the
    // rendered region — so the gap precedes the inline removed block.
    expect(html).toContain('diff-rendered-gap');
    const gap = html.indexOf('diff-rendered-gap');
    const removed = html.indexOf('diff-rendered-removed');
    expect(gap).toBeLessThan(removed);
  });
});

describe('renderDiffMarked deletion placement', () => {
  it('renders a removed run inline at its original position, not at the top', () => {
    const content = `# Title

Para A.

Para B.
`;
    const file = fileFromLines(1, [
      ['c', '# Title'],
      ['c', ''],
      ['c', 'Para A.'],
      ['c', ''],
      ['-', 'Old para.'],
      ['-', ''],
      ['c', 'Para B.'],
    ]);
    const html = renderDiffMarked(content, additionRuns(file), deletionRuns(file));
    expect(html).toContain('diff-rendered-removed');
    // The removed text lands between Para A and Para B — not hoisted to the top.
    const del = html.indexOf('Old para.');
    expect(del).toBeGreaterThan(html.indexOf('Para A'));
    expect(del).toBeLessThan(html.indexOf('Para B'));
  });

  it('renders end-of-file deletions after all content', () => {
    const content = `Only line.\n`;
    const file = fileFromLines(1, [
      ['c', 'Only line.'],
      ['-', 'trailing gone'],
    ]);
    const html = renderDiffMarked(content, additionRuns(file), deletionRuns(file));
    expect(html.indexOf('Only line')).toBeLessThan(html.indexOf('trailing gone'));
  });

  it('HTML-escapes removed line content', () => {
    const content = `Kept.\n`;
    const file = fileFromLines(1, [
      ['-', '<script>alert(1)</script>'],
      ['c', 'Kept.'],
    ]);
    const html = renderDiffMarked(content, additionRuns(file), deletionRuns(file));
    expect(html).toContain('&lt;script&gt;');
    expect(html).not.toContain('<script>');
  });
});
