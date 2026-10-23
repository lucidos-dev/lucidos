/**
 * Enter COMMITS an IME candidate, and must never also send or submit.
 *
 * A Japanese, Chinese or Korean user types a phrase and presses Enter to accept
 * what the IME is offering. The browser dispatches that keydown before
 * `compositionend`. An Enter branch with no composition guard fired on it: the
 * composer sent half-converted text, and a rename saved it as the title.
 * `preventDefault` also fought the IME's own commit.
 *
 * `isImeComposingKey` is the shared guard. The composer's own handler is a
 * closure over component state, so a test cannot reach it. The prompt dialog,
 * where a thread is renamed, decides through `promptEnterSubmits`.
 */
import { describe, it, expect } from 'vitest';
import { isImeComposingKey } from '../../../utils/ime';
import { promptEnterSubmits } from '../../shared/PromptDialog';

describe('isImeComposingKey', () => {
  it('reads the standard isComposing flag', () => {
    expect(isImeComposingKey({ isComposing: true, keyCode: 13 })).toBe(true);
  });

  it('reads the legacy 229 keyCode, which is all some browsers send', () => {
    expect(isImeComposingKey({ isComposing: false, keyCode: 229 })).toBe(true);
  });

  it('is false for an ordinary Enter, so sending still works', () => {
    expect(isImeComposingKey({ isComposing: false, keyCode: 13 })).toBe(false);
  });
});

describe('promptEnterSubmits', () => {
  const enter = { key: 'Enter', isComposing: false, keyCode: 13 };

  it('does NOT submit on the Enter that commits a candidate', () => {
    expect(promptEnterSubmits({ ...enter, isComposing: true }, 'INPUT', false)).toBe(false);
  });

  it('does NOT submit when only the legacy 229 keyCode marks the composition', () => {
    expect(promptEnterSubmits({ ...enter, keyCode: 229 }, 'INPUT', false)).toBe(false);
  });

  it('still submits on an ordinary Enter in the single-line field', () => {
    expect(promptEnterSubmits(enter, 'INPUT', false)).toBe(true);
  });

  it('leaves Enter to a focused button and to a multiline textarea', () => {
    expect(promptEnterSubmits(enter, 'BUTTON', false)).toBe(false);
    expect(promptEnterSubmits(enter, 'TEXTAREA', true)).toBe(false);
  });

  it('ignores any other key', () => {
    expect(promptEnterSubmits({ ...enter, key: 'a' }, 'INPUT', false)).toBe(false);
  });
});
