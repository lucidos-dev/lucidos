/**
 * `composeHasContent` decides whether there is anything to send. It drives the
 * section-button vs Send-button choice in PromptInput AND `submit()`'s own
 * dispatch, which is the whole point of it: two readings of that question is
 * what an enabled Send whose press does nothing is made of.
 *
 * Regression 1: pasting an image used to flash the Save button (for review-
 * section threads) before Send. Reason: the upload happens in the background.
 * The image lives in `pendingUploads` until the server returns the hash, only
 * then moving to `attachedImages`. The original predicate
 * `text || attachedImages` was false during that window, so the prompt
 * looked empty and the section-button slot won, rendering Save where Send
 * should have been.
 *
 * Regression 2, the reason this takes the raw text and an in-flight BOOLEAN:
 * the face used to light from `text.length > 0` while the send refused on
 * `text.trim()`, and a `failed` upload counted as content while never becoming
 * a send. Both drew a live Send button that did nothing and said nothing.
 * See docs/plans/2026-08-28-a-swallowed-tap-says-so.md.
 */
import { describe, it, expect } from 'vitest';
import { composeHasContent } from '../PromptInput';

describe('composeHasContent', () => {
  it('false when nothing typed and no images', () => {
    expect(composeHasContent('', 0, false)).toBe(false);
  });

  it('true when the user has typed text', () => {
    expect(composeHasContent('hello', 0, false)).toBe(true);
  });

  it('true when an image is attached', () => {
    expect(composeHasContent('', 1, false)).toBe(true);
  });

  // Regression: pasting an image must immediately count as content so the
  // Save button (review-section default) yields to Send during the upload
  // window. Clicking Send now queues the send until the hash lands.
  it('true while an image upload is in flight, so Save yields to Send', () => {
    expect(composeHasContent('', 0, true)).toBe(true);
  });

  it('true when text and an in-flight upload coexist', () => {
    expect(composeHasContent('hello', 0, true)).toBe(true);
  });

  it('false for a draft of nothing but whitespace', () => {
    // The send trims, so there is nothing here to send. Lighting Send from the
    // untrimmed length drew a button whose press returned in silence.
    expect(composeHasContent('   ', 0, false)).toBe(false);
    expect(composeHasContent('\n\t ', 0, false)).toBe(false);
  });

  it('false for a FAILED upload alone, which will never become a send', () => {
    // The caller passes `hasInFlightUploads`, which counts only `uploading`.
    // A failed entry still draws in the strip so it can be removed or retried,
    // and it is not a reason to light Send.
    expect(composeHasContent('', 0, false)).toBe(false);
  });
});
