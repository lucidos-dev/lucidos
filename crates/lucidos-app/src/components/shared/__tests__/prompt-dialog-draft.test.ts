import { describe, it, expect } from 'vitest';
import { promptInputSeed } from '../PromptDialog';

// The prompt's input is uncontrolled (no re-render per keystroke), so its text
// lives only in the DOM node and a REMOUNT seeds a fresh one. That is right when
// a new prompt replaces this one and wrong when it is the same prompt landing in
// a new place: `OverlayLayer` re-parents the whole overlay group into and out of
// a fullscreen app panel, so leaving fullscreen mid-answer reset what the reader
// had written. The draft is keyed on the prompt's `resolve` closure, which is
// fresh per showPrompt call, so "same question" and "different question" are
// distinguishable.

describe('promptInputSeed', () => {
  const resolve = () => {};
  const other = () => {};

  it('seeds from the prompt default when there is no draft', () => {
    expect(promptInputSeed(null, resolve, 'the default')).toBe('the default');
  });

  it('seeds empty when there is neither a draft nor a default', () => {
    expect(promptInputSeed(null, resolve, undefined)).toBe('');
  });

  // The fullscreen re-parent: same question, new DOM node.
  it('restores what the reader typed when the same prompt remounts', () => {
    expect(promptInputSeed({ resolve, value: 'half an answer' }, resolve, 'the default'))
      .toBe('half an answer');
  });

  // A second showPrompt REPLACES a visible one. It must not inherit the
  // previous question's text, which is the bug the draft could have introduced.
  it('ignores a draft belonging to a different prompt', () => {
    expect(promptInputSeed({ resolve: other, value: 'previous answer' }, resolve, 'the default'))
      .toBe('the default');
  });

  // Deleting everything is an answer: the empty draft must win over the default,
  // or clearing the field and leaving fullscreen would put the default back.
  it('keeps a deliberately emptied field empty', () => {
    expect(promptInputSeed({ resolve, value: '' }, resolve, 'the default')).toBe('');
  });
});
