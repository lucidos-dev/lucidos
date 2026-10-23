import { describe, it, expect } from 'vitest';
import { createHeadingSlugger } from './headingSlug';

describe('createHeadingSlugger', () => {
  it('matches the slug GitHub gives a heading with punctuation', () => {
    const slug = createHeadingSlugger();
    expect(slug('Auth tier compatibility (API key vs subscription / OAuth)'))
      .toBe('auth-tier-compatibility-api-key-vs-subscription--oauth');
  });

  it('keeps letters, digits, underscores and hyphens, in any script', () => {
    const slug = createHeadingSlugger();
    expect(slug('Step 2: run_tests -- fast')).toBe('step-2-run_tests----fast');
    expect(slug('Café Ærø')).toBe('café-ærø');
  });

  it('numbers a repeated heading the way github-slugger does', () => {
    const slug = createHeadingSlugger();
    expect(slug('Usage')).toBe('usage');
    expect(slug('Usage')).toBe('usage-1');
    expect(slug('Usage')).toBe('usage-2');
  });

  it('steps past a slug an earlier heading already took', () => {
    const slug = createHeadingSlugger();
    expect(slug('Foo')).toBe('foo');
    expect(slug('Foo 1')).toBe('foo-1');
    expect(slug('Foo')).toBe('foo-2');
  });

  it('starts every document afresh', () => {
    createHeadingSlugger()('Intro');
    expect(createHeadingSlugger()('Intro')).toBe('intro');
  });
});
