import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import type { VNode } from 'preact';
import { fitsSideBySide, SIDE_BY_SIDE_MIN_REM } from './DiffView';
import { SideBySideDiff } from './sideBySideDiff';
import { LineNumberedCode } from './LineNumberedCode';
import type { DiffFile } from '../../store/store';

const REM = 16;

describe('fitsSideBySide', () => {
  it('fits at the threshold', () => {
    expect(fitsSideBySide(SIDE_BY_SIDE_MIN_REM * REM, REM)).toBe(true);
  });

  it('does not fit just below it', () => {
    expect(fitsSideBySide(SIDE_BY_SIDE_MIN_REM * REM - 1, REM)).toBe(false);
  });

  it('fits comfortably above it', () => {
    expect(fitsSideBySide(SIDE_BY_SIDE_MIN_REM * REM + 200, REM)).toBe(true);
  });

  it('does not fit at a phone width', () => {
    expect(fitsSideBySide(390, REM)).toBe(false);
  });

  it('scales with the root font size', () => {
    const width = SIDE_BY_SIDE_MIN_REM * REM;
    expect(fitsSideBySide(width, REM)).toBe(true);
    expect(fitsSideBySide(width, REM * 1.5)).toBe(false);
  });

  it('treats an unmeasured container as room, so the first paint does not flash', () => {
    expect(fitsSideBySide(0, REM)).toBe(true);
  });
});

describe('SideBySideDiff renders both columns through LineNumberedCode', () => {
  const file: DiffFile = {
    path: 'src/main.rs',
    status: 'modified',
    hunks: [{
      old_start: 1,
      old_count: 1,
      new_start: 1,
      new_count: 1,
      lines: [{ type: 'deletion', content: 'a' }, { type: 'addition', content: 'b' }],
    }],
  };

  const root = SideBySideDiff({ file }) as VNode<{ children: VNode<Record<string, unknown>>[] }>;
  const sides = root.props.children;

  it('renders exactly two columns', () => {
    expect(sides).toHaveLength(2);
  });

  it('uses LineNumberedCode for both', () => {
    for (const side of sides) {
      const code = side.props.children as VNode<Record<string, unknown>>;
      expect(code.type).toBe(LineNumberedCode);
    }
  });

  it('turns the file-level line selection off in both', () => {
    for (const side of sides) {
      const code = side.props.children as VNode<Record<string, unknown>>;
      expect(code.props.selection).toBe('none');
    }
  });
});

describe('exactly one DiffView publishes the measured fit', () => {
  const dir = new URL('.', import.meta.url);
  const diffView = readFileSync(new URL('DiffView.tsx', dir), 'utf8');
  const repoPreview = readFileSync(new URL('RepoFilePreview.tsx', dir), 'utf8');
  const filesView = readFileSync(new URL('RepoFilesView.tsx', dir), 'utf8');

  it('writes the signal in exactly one place, behind the opt-in', () => {
    expect(diffView.match(/diffFitsSideBySide\.value\s*=/g)).toHaveLength(1);
    const effect = diffView.split('useLayoutEffect(')[1];
    expect(effect.split('diffFitsSideBySide.value =')[0]).toContain('measureFit');
  });

  it('is opted into by the file preview and by nothing else', () => {
    expect(repoPreview).toContain('measureFit');
    expect(filesView).not.toContain('measureFit');
  });
});
