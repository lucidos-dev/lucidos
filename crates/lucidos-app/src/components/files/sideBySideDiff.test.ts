import { describe, it, expect, vi } from 'vitest';
import type { DiffFile, DiffHunk, DiffLine } from '../../store/store';

// The real `escapeHtml` round-trips through a DOM element, and the test infra
// has no jsdom (its `document` stub returns '' for `innerHTML`), so every line
// would come back empty and the mapping assertions below could not fail. A
// marker escaper keeps them honest AND still proves the text goes through the
// escaper rather than being injected raw.
vi.mock('../../utils/escapeHtml', () => ({
  escapeHtml: (text: string) => `esc(${text})`,
  stripHtml: (html: string) => html,
}));

const { sideBySideRows, sideBySideColumns, hunkHeader } = await import('./sideBySideDiff');

const ctx = (content: string): DiffLine => ({ type: 'context', content });
const del = (content: string): DiffLine => ({ type: 'deletion', content });
const add = (content: string): DiffLine => ({ type: 'addition', content });

function hunk(lines: DiffLine[], oldStart = 1, newStart = 1): DiffHunk {
  return {
    old_start: oldStart,
    old_count: lines.filter(l => l.type !== 'addition').length,
    new_start: newStart,
    new_count: lines.filter(l => l.type !== 'deletion').length,
    lines,
  };
}

/** The columns line up only if every row has a cell (or a filler) on both
 *  sides, which `sideBySideRows` guarantees by construction; asserted on every
 *  shape below rather than stated once. */
function numbers(rows: ReturnType<typeof sideBySideRows>) {
  return {
    left: rows.map(r => r.left?.num ?? null),
    right: rows.map(r => r.right?.num ?? null),
  };
}

describe('sideBySideRows: context', () => {
  it('puts a context line on both sides of one row', () => {
    const rows = sideBySideRows(hunk([ctx('a'), ctx('b')], 10, 20));
    expect(numbers(rows)).toEqual({ left: [10, 11], right: [20, 21] });
    expect(rows.every(r => r.left?.kind === 'context' && r.right?.kind === 'context')).toBe(true);
  });
});

describe('sideBySideRows: a pure insertion', () => {
  it('fills the left side, and does not advance the old line number', () => {
    const rows = sideBySideRows(hunk([ctx('a'), add('x'), add('y'), ctx('b')], 1, 1));
    expect(numbers(rows)).toEqual({
      left: [1, null, null, 2],
      right: [1, 2, 3, 4],
    });
  });

  it('is all filler on the left for an added file', () => {
    const rows = sideBySideRows(hunk([add('x'), add('y'), add('z')], 0, 1));
    expect(numbers(rows)).toEqual({ left: [null, null, null], right: [1, 2, 3] });
  });
});

describe('sideBySideRows: a pure deletion', () => {
  it('fills the right side, and does not advance the new line number', () => {
    const rows = sideBySideRows(hunk([ctx('a'), del('x'), del('y'), ctx('b')], 1, 1));
    expect(numbers(rows)).toEqual({
      left: [1, 2, 3, 4],
      right: [1, null, null, 2],
    });
  });

  it('is all filler on the right for a deleted file', () => {
    const rows = sideBySideRows(hunk([del('x'), del('y')], 1, 0));
    expect(numbers(rows)).toEqual({ left: [1, 2], right: [null, null] });
  });
});

describe('sideBySideRows: a replacement', () => {
  // The point of the view: the line before and the line after sit on one row.
  it('pairs a balanced run index for index', () => {
    const rows = sideBySideRows(hunk([ctx('a'), del('x'), del('y'), add('X'), add('Y'), ctx('b')], 1, 1));
    expect(numbers(rows)).toEqual({
      left: [1, 2, 3, 4],
      right: [1, 2, 3, 4],
    });
    expect(rows[1].left?.kind).toBe('change');
    expect(rows[1].right?.kind).toBe('change');
  });

  it('pads the short side of an unbalanced run', () => {
    const rows = sideBySideRows(hunk([del('x'), add('X'), add('Y'), add('Z')], 1, 1));
    expect(numbers(rows)).toEqual({
      left: [1, null, null],
      right: [1, 2, 3],
    });
  });

  it('pads the other way when deletions outnumber additions', () => {
    const rows = sideBySideRows(hunk([del('x'), del('y'), del('z'), add('X')], 1, 1));
    expect(numbers(rows)).toEqual({
      left: [1, 2, 3],
      right: [1, null, null],
    });
  });

  // Two runs separated by context must not merge: the second run's deletions
  // belong beside the second run's additions, not the first's.
  it('closes a run at the context line between two of them', () => {
    const rows = sideBySideRows(hunk([del('a'), add('A'), ctx('keep'), del('b'), add('B')], 1, 1));
    expect(numbers(rows)).toEqual({
      left: [1, 2, 3],
      right: [1, 2, 3],
    });
    expect(rows[1].left?.kind).toBe('context');
  });
});

describe('sideBySideRows: alignment', () => {
  it('emits one row per pairing, so the two columns are always the same length', () => {
    const shapes: DiffLine[][] = [
      [ctx('a')],
      [add('x'), add('y')],
      [del('x'), del('y')],
      [del('x'), add('X'), add('Y')],
      [ctx('a'), del('x'), add('X'), ctx('b'), add('y')],
      [],
    ];
    for (const lines of shapes) {
      const rows = sideBySideRows(hunk(lines));
      const { left, right } = numbers(rows);
      expect(left).toHaveLength(right.length);
      expect(rows.every(r => r.left !== null || r.right !== null)).toBe(true);
    }
  });

  it('indexes each cell back to its line in the hunk, for the highlighter', () => {
    const lines = [ctx('a'), del('x'), add('X')];
    const rows = sideBySideRows(hunk(lines));
    expect(rows[0].left?.index).toBe(0);
    expect(rows[1].left?.index).toBe(1);
    expect(rows[1].right?.index).toBe(2);
  });
});

describe('hunkHeader', () => {
  it('reads the same as the unified view header', () => {
    expect(hunkHeader({ old_start: 12, old_count: 3, new_start: 14, new_count: 5, lines: [] }))
      .toBe('@@ -12,3 +14,5 @@');
  });
});

describe('sideBySideColumns', () => {
  function file(hunks: DiffHunk[], path = 'src/main.rs'): DiffFile {
    return { path, status: 'modified', hunks };
  }

  it('produces two columns of equal length, header rows included', () => {
    const { left, right } = sideBySideColumns(file([hunk([ctx('a'), del('x'), add('X')])]), 'rs');
    expect(left).toHaveLength(right.length);
    // One `@@` header plus two paired rows.
    expect(left).toHaveLength(3);
    expect(left[0].cls).toBe('side-by-side-diff-hunk-header');
    expect(right[0].cls).toBe('side-by-side-diff-hunk-header');
  });

  it('stays aligned across several hunks', () => {
    const { left, right } = sideBySideColumns(
      file([hunk([ctx('a'), add('x')], 1, 1), hunk([del('y'), del('z')], 40, 41)]),
      'rs',
    );
    expect(left).toHaveLength(right.length);
    expect(left.map(r => r.num)).toEqual([null, 1, null, null, 40, 41]);
    expect(right.map(r => r.num)).toEqual([null, 1, 2, null, null, null]);
  });

  it('tints a deletion on the left and an addition on the right', () => {
    const { left, right } = sideBySideColumns(file([hunk([del('x'), add('X')])]), 'rs');
    expect(left[1].cls).toBe('side-by-side-diff-deletion');
    expect(right[1].cls).toBe('side-by-side-diff-addition');
  });

  it('leaves a context row untinted on both sides', () => {
    const { left, right } = sideBySideColumns(file([hunk([ctx('a')])]), 'rs');
    expect(left[1].cls).toBeUndefined();
    expect(right[1].cls).toBeUndefined();
  });

  it('marks the missing side of an unbalanced run as filler', () => {
    const { left } = sideBySideColumns(file([hunk([add('x')])]), 'rs');
    expect(left[1]).toEqual({ html: '', num: null, cls: 'side-by-side-diff-filler' });
  });

  // The highlighter runs over the whole hunk at once (so a multi-line construct
  // survives being split into rows) and each cell indexes back into it. A
  // mis-indexed cell would show a neighbouring line's text, which is the worst
  // possible bug in a diff.
  it('gives each side the text of its own line', () => {
    const { left, right } = sideBySideColumns(file([hunk([del('gone'), add('kept')])]), 'txt');
    expect(left[1].html).toBe('esc(gone)');
    expect(right[1].html).toBe('esc(kept)');
  });

  // Off-by-one in the index mapping is the worst bug this view can have: every
  // row would show a neighbouring line's text and read as a plausible diff.
  it('keeps each cell pointing at its own line across a long run', () => {
    const only = hunk([ctx('a'), del('d1'), del('d2'), del('d3'), add('a1'), add('a2'), ctx('b')]);
    const header = `esc(${hunkHeader(only)})`;
    const { left, right } = sideBySideColumns(file([only]), 'txt');
    expect(left.map(r => r.html)).toEqual([header, 'esc(a)', 'esc(d1)', 'esc(d2)', 'esc(d3)', 'esc(b)']);
    expect(right.map(r => r.html)).toEqual([header, 'esc(a)', 'esc(a1)', 'esc(a2)', '', 'esc(b)']);
  });

  it('routes the content through the escaper rather than injecting it raw', () => {
    const { right } = sideBySideColumns(file([hunk([add('<script>x</script>')])], 'notes.txt'), 'txt');
    expect(right[1].html).toBe('esc(<script>x</script>)');
  });
});
