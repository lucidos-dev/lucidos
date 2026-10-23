import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import type { VNode } from 'preact';
import { fitsSideBySide, SIDE_BY_SIDE_MIN_REM } from './DiffView';
import { SideBySideDiff } from './sideBySideDiff';
import { LineNumberedCode } from './LineNumberedCode';
import type { DiffFile } from '../../store/store';

const REM = 16;

/** Two columns of unwrapped code need real width. The threshold is measured off
 *  the diff's own container rather than the viewport, because the content pane
 *  is resizable: a desktop user can drag the split until two columns no longer
 *  fit, and a phone never has room at all. */
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

  // The threshold is in `rem`, so a user who has scaled the UI up needs
  // proportionally more pixels for the same two columns.
  it('scales with the root font size', () => {
    const width = SIDE_BY_SIDE_MIN_REM * REM;
    expect(fitsSideBySide(width, REM)).toBe(true);
    expect(fitsSideBySide(width, REM * 1.5)).toBe(false);
  });

  // Before the ResizeObserver has run, the container reports 0. Reading that as
  // "no room" would render unified for one frame and then swap, flashing the
  // wrong view at every open.
  it('treats an unmeasured container as room, so the first paint does not flash', () => {
    expect(fitsSideBySide(0, REM)).toBe(true);
  });
});

/** The side-by-side columns must be the file preview's own renderer, not a third
 *  implementation of line numbering and row markup, and they must not
 *  participate in the file-level line selection (their numbers are the OLD
 *  file's on the left and the NEW file's on the right). */
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

  // Hookless at its own level, so it can be invoked directly and its vnode tree
  // walked (the same approach as ThreadFilterPanel.test.tsx).
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

/** `diffFitsSideBySide` is a single global, and `DiffView` is NOT a
 *  single-instance component: `InlineDiffList` stacks one per changed file. Only
 *  the file-preview diff (the surface the header's toggle acts on) may measure
 *  and publish, or the rest become redundant ResizeObservers racing to set the
 *  same value off containers the toggle does not act on. The gate is a
 *  `useLayoutEffect`, so this is a source scan rather than a render. */
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
