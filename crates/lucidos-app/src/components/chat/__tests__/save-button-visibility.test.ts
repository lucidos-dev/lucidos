import { describe, it, expect } from 'vitest';
import { shouldShowSaveButton } from '../PromptInput';

describe('shouldShowSaveButton', () => {
  it('shows for unsaved threads only when canArchive', () => {
    expect(shouldShowSaveButton(false, true)).toBe(true);
    expect(shouldShowSaveButton(false, false)).toBe(false);
  });

  // Bug reproducer: saved+archived threads have canArchive=false because
  // resolveActions returns [] for non-inbox sections. Without this rule the
  // user has no way to unsave once a saved thread auto-archives.
  it('always shows for saved threads, regardless of canArchive', () => {
    expect(shouldShowSaveButton(true, true)).toBe(true);
    expect(shouldShowSaveButton(true, false)).toBe(true);
  });
});
