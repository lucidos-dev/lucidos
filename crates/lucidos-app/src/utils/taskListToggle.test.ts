import { describe, it, expect } from 'vitest';
import { toggleTaskListCheckbox, countTaskListItems } from './taskListToggle';

describe('toggleTaskListCheckbox', () => {
  it('flips an unchecked box to checked', () => {
    const src = '- [ ] Buy milk\n- [ ] Walk dog\n';
    expect(toggleTaskListCheckbox(src, 0)).toBe('- [x] Buy milk\n- [ ] Walk dog\n');
  });

  it('flips a checked box back to unchecked, both case forms', () => {
    const src = '- [x] Buy milk\n- [X] Walk dog\n';
    expect(toggleTaskListCheckbox(src, 0)).toBe('- [ ] Buy milk\n- [X] Walk dog\n');
    expect(toggleTaskListCheckbox(src, 1)).toBe('- [x] Buy milk\n- [ ] Walk dog\n');
  });

  it('counts nested and ordered task items in source order', () => {
    const src = [
      '- [ ] Top',
      '  - [ ] Nested one',
      '  - [x] Nested two',
      '1. [ ] Ordered',
      '2) [ ] Ordered paren',
    ].join('\n');
    // index 0 Top, 1 Nested one, 2 Nested two, 3 Ordered, 4 Ordered paren
    expect(toggleTaskListCheckbox(src, 3)).toContain('1. [x] Ordered');
    expect(toggleTaskListCheckbox(src, 4)).toContain('2) [x] Ordered paren');
  });

  it('skips a fenced code block containing a fake task item (backtick fence)', () => {
    const src = [
      '- [ ] Real one',
      '```',
      '- [ ] fake, inside a code block',
      '```',
      '- [ ] Real two',
    ].join('\n');
    const updated = toggleTaskListCheckbox(src, 1);
    expect(updated).not.toBeNull();
    const lines = updated!.split('\n');
    expect(lines[0]).toBe('- [ ] Real one');
    expect(lines[2]).toBe('- [ ] fake, inside a code block');
    expect(lines[4]).toBe('- [x] Real two');
  });

  it('skips a tilde-fenced code block too, and requires a matching close length', () => {
    const src = [
      '~~~~',
      '- [ ] fake',
      '~~~',
      'still inside: the shorter close did not match',
      '~~~~',
      '- [ ] Real',
    ].join('\n');
    const updated = toggleTaskListCheckbox(src, 0);
    expect(updated).not.toBeNull();
    expect(updated!.split('\n')[5]).toBe('- [x] Real');
  });

  it('skips a CRLF frontmatter block instead of corrupting it', () => {
    // Pins a real bug: frontmatterLineCount's lines keep a trailing `\r`
    // (needed so splitting on '\n' alone stays byte-identical), and the
    // shared fence/key-line regexes must still recognize such a line.
    const src = '---\r\ntitle: My Doc\r\ntodo:\r\n  - [ ] milk\r\n---\r\n- [ ] real task\r\n';
    const updated = toggleTaskListCheckbox(src, 0);
    expect(updated).toBe('---\r\ntitle: My Doc\r\ntodo:\r\n  - [ ] milk\r\n---\r\n- [x] real task\r\n');
  });

  it('skips a frontmatter block, even one containing a task-item-shaped line', () => {
    const src = [
      '---',
      'title: notes',
      '- [ ] not a real task item',
      '---',
      '- [ ] First real item',
    ].join('\n');
    const updated = toggleTaskListCheckbox(src, 0);
    expect(updated).not.toBeNull();
    const lines = updated!.split('\n');
    expect(lines[2]).toBe('- [ ] not a real task item'); // untouched
    expect(lines[4]).toBe('- [x] First real item');
  });

  it('counts a quoted task item in order, without shifting later indices', () => {
    const src = [
      '> - [ ] Quoted one',
      '> - [x] Quoted two',
      '- [ ] Top-level',
    ].join('\n');
    // Rendered checkbox order: Quoted one (0), Quoted two (1), Top-level (2).
    expect(toggleTaskListCheckbox(src, 0)).toContain('> - [x] Quoted one');
    expect(toggleTaskListCheckbox(src, 1)).toContain('> - [ ] Quoted two');
    const updated = toggleTaskListCheckbox(src, 2);
    expect(updated).not.toBeNull();
    const lines = updated!.split('\n');
    expect(lines[0]).toBe('> - [ ] Quoted one'); // untouched
    expect(lines[1]).toBe('> - [x] Quoted two'); // untouched
    expect(lines[2]).toBe('- [x] Top-level');
  });

  it('counts a nested-blockquote task item and preserves its markers', () => {
    const src = '> > - [ ] Twice-quoted\n- [ ] Top-level\n';
    const updated = toggleTaskListCheckbox(src, 0);
    expect(updated).toBe('> > - [x] Twice-quoted\n- [ ] Top-level\n');
  });

  it('skips a fenced code block inside a blockquote', () => {
    const src = [
      '> - [ ] Real',
      '> ```',
      '> - [ ] fake',
      '> ```',
      '- [ ] Top-level',
    ].join('\n');
    const updated = toggleTaskListCheckbox(src, 1);
    expect(updated).not.toBeNull();
    const lines = updated!.split('\n');
    expect(lines[2]).toBe('> - [ ] fake'); // untouched
    expect(lines[4]).toBe('- [x] Top-level');
  });

  it('returns null when the index is out of range', () => {
    const src = '- [ ] Only one\n';
    expect(toggleTaskListCheckbox(src, 1)).toBeNull();
    expect(toggleTaskListCheckbox(src, -1)).toBeNull();
  });

  it('preserves CRLF line endings exactly, touching only the flipped char', () => {
    const src = '- [ ] One\r\n- [ ] Two\r\n';
    const updated = toggleTaskListCheckbox(src, 1);
    expect(updated).toBe('- [ ] One\r\n- [x] Two\r\n');
  });

  it('preserves a missing trailing newline', () => {
    const src = '- [ ] Only one';
    expect(toggleTaskListCheckbox(src, 0)).toBe('- [x] Only one');
  });

  it('leaves everything else byte-identical, change confined to one line', () => {
    const src = [
      '# Notes',
      '',
      'Some prose with *emphasis* and a [link](https://example.com).',
      '',
      '- [ ] First',
      '- [x] Second',
      '',
      '> A blockquote, untouched.',
    ].join('\n');
    const updated = toggleTaskListCheckbox(src, 1);
    expect(updated).not.toBeNull();
    const before = src.split('\n');
    const after = updated!.split('\n');
    expect(after.length).toBe(before.length);
    for (let i = 0; i < before.length; i++) {
      if (i === 5) continue; // the toggled line
      expect(after[i]).toBe(before[i]);
    }
    expect(after[5]).toBe('- [ ] Second');
  });
});

describe('countTaskListItems', () => {
  it('matches the number of items toggleTaskListCheckbox can reach', () => {
    const src = '- [ ] One\n- [x] Two\n  - [ ] Nested\n';
    expect(countTaskListItems(src)).toBe(3);
  });

  it('returns 0 for a file with no task items', () => {
    expect(countTaskListItems('# Notes\n\nJust prose.\n')).toBe(0);
  });

  it('does not count a line indented 4+ spaces (an indented code block)', () => {
    // A known gap: `marked` renders this as a code block with no checkbox.
    // The line-scan has no block parser and still matches it, so this
    // undercounts relative to what `marked` would show once a real task item
    // follows. That mismatch is what a caller compares against the DOM count
    // to detect, rather than toggling the wrong line (see MarkdownDocument.tsx).
    const src = '    - [ ] indented, not really a task\n\n- [ ] Real\n';
    expect(countTaskListItems(src)).toBe(2); // marked would render only 1 checkbox
  });
});
