import { describe, expect, it } from 'vitest';
import { PREF_TITLE_STYLE } from '@lucidos/preference-catalog';
import { titleStyleCommit, titleStyleOneLine } from './titleStyle';

describe('titleStyleOneLine', () => {
  it('joins pasted lines with one space', () => {
    expect(titleStyleOneLine('kebab-case\n  at most 4 words\r\nno dates')).toBe(
      'kebab-case at most 4 words no dates',
    );
  });

  it('leaves single-line text alone', () => {
    expect(titleStyleOneLine('  kebab-case ')).toBe('  kebab-case ');
  });
});

const MAX = PREF_TITLE_STYLE.maxChars;

describe('titleStyleCommit', () => {
  it('does nothing when the trimmed text matches what is stored', () => {
    expect(titleStyleCommit('  kebab-case ', 'kebab-case', MAX)).toEqual({ kind: 'none' });
    expect(titleStyleCommit('', '', MAX)).toEqual({ kind: 'none' });
    expect(titleStyleCommit('   ', '', MAX)).toEqual({ kind: 'none' });
  });

  it('saves new text trimmed', () => {
    expect(titleStyleCommit('  kebab-case ', '', MAX)).toEqual({ kind: 'save', value: 'kebab-case' });
  });

  it('clears when a stored style is blanked, never writing an empty value', () => {
    expect(titleStyleCommit('  ', 'kebab-case', MAX)).toEqual({ kind: 'clear' });
  });

  it('refuses text past the catalog bound and counts characters, not bytes', () => {
    expect(titleStyleCommit('å'.repeat(MAX), '', MAX).kind).toBe('save');
    expect(titleStyleCommit('a'.repeat(MAX + 1), '', MAX)).toEqual({ kind: 'too-long' });
  });
});
