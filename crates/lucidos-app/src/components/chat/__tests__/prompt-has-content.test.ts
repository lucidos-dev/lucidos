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

  it('true while an image upload is in flight, so Save yields to Send', () => {
    expect(composeHasContent('', 0, true)).toBe(true);
  });

  it('true when text and an in-flight upload coexist', () => {
    expect(composeHasContent('hello', 0, true)).toBe(true);
  });

  it('false for a draft of nothing but whitespace', () => {
    expect(composeHasContent('   ', 0, false)).toBe(false);
    expect(composeHasContent('\n\t ', 0, false)).toBe(false);
  });

  it('false for a FAILED upload alone, which will never become a send', () => {
    expect(composeHasContent('', 0, false)).toBe(false);
  });
});
