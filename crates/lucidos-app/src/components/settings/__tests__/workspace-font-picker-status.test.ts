import { describe, expect, it } from 'vitest';
import { chosenFilesSummary } from '../WorkspaceFontsSection';
import type { FontFileChoice } from '../../../store/actions/workspaceFonts';

function choice(name: string): FontFileChoice {
  return { file: new File(['x'], name), weight: '400', style: 'normal' };
}

describe('the font picker status', () => {
  it('says nothing is chosen before a pick', () => {
    expect(chosenFilesSummary([])).toBe('No file chosen');
  });

  it('names a single file', () => {
    expect(chosenFilesSummary([choice('BrandSans-Bold.woff2')])).toBe('BrandSans-Bold.woff2');
  });

  it('counts several files', () => {
    const files = ['A.woff2', 'B.woff2', 'C.woff2'].map(choice);
    expect(chosenFilesSummary(files)).toBe('3 files');
  });
});
