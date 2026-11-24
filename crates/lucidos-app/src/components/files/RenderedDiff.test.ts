// @vitest-environment jsdom

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
    const runs = additionRuns(additionAt(11, 4));
    const html = renderDiffMarked(content, runs);

    expect(html).toContain('diff-rendered-block-added');
    expect(html).not.toMatch(/<h1[^>]*class="[^"]*diff-rendered-(added|changed)/);
    expect(html).not.toMatch(/<h2[^>]*class="[^"]*diff-rendered-(added|changed)/);
    expect(html).not.toMatch(/<p[^>]*class="[^"]*diff-rendered-(added|changed)/);
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
    const runs = additionRuns(additionAt(6, 1));
    const html = renderDiffMarked(content, runs);

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
      ['-', 'gone top'],
      ['c', 'keep'],
      ['+', 'new'],
      ['-', 'gone mid'],
      ['c', 'tail'],
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
    const html = renderDiffMarked(content, [], [], [{ start: 5, end: 9 }]);
    expect(html).toContain('Section A');
    expect(html).toContain('Changed para.');
    expect(html).toContain('Section B');
    expect(html).not.toContain('Title');
    expect(html).not.toContain('Intro para.');
    expect(html).not.toContain('Unchanged tail.');
    expect(html.match(/diff-rendered-gap/g)).toHaveLength(2);
  });

  it('keeps an inline deletion when scoped, with the leading gap above it', () => {
    const dels = [{ anchor: 7, lines: ['Removed old line.'] }];
    const html = renderDiffMarked(content, [], dels, [{ start: 5, end: 9 }]);
    expect(html).toContain('diff-rendered-removed');
    expect(html).toContain('Removed old line.');
    expect(html).toContain('Section A');
    expect(html).toContain('Changed para.');
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

describe('renderDiffMarked email autolinks inside a path', () => {
  it('leaves an email-shaped path segment as plain text, with no marker left behind', () => {
    const html = renderDiffMarked('See /Users/me.x@example.com/p/a.dmg and write me.x@example.com', []);
    expect(html).toContain('/Users/me.x@example.com/p/a.dmg');
    expect(html.match(/mailto:/g)).toHaveLength(1);
    expect(html).not.toContain('data-bare-email');
  });
});
