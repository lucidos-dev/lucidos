/**
 * `composeHasContent` decides whether the prompt is "actively composing".
 * It drives the section-button vs Send-button choice in PromptInput.
 *
 * Regression: pasting an image used to flash the Save button (for review-
 * section threads) before Send. Reason — the upload happens in the
 * background: the image lives in `pendingUploads` until the server returns
 * the hash, only then moving to `attachedImages`. The original predicate
 * `text || attachedImages` was false during that window, so the prompt
 * looked empty and `getPromptSectionButtons('review', ...)` won, rendering
 * Save in the slot that should have been Send.
 */
import { describe, it, expect } from 'vitest';
import { composeHasContent } from '../PromptInput';

describe('composeHasContent', () => {
  it('false when nothing typed and no images', () => {
    expect(composeHasContent(false, 0, 0)).toBe(false);
  });

  it('true when the user has typed text', () => {
    expect(composeHasContent(true, 0, 0)).toBe(true);
  });

  it('true when an image is attached', () => {
    expect(composeHasContent(false, 1, 0)).toBe(true);
  });

  // Regression — pasting an image must immediately count as content so the
  // Save button (review-section default) yields to Send during the upload
  // window. Send is then disabled by `uploadsBlocking` until the hash lands.
  it('true while an image upload is pending — Save must yield to Send', () => {
    expect(composeHasContent(false, 0, 1)).toBe(true);
  });

  it('true when text and pending upload coexist', () => {
    expect(composeHasContent(true, 0, 1)).toBe(true);
  });
});
