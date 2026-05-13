import { describe, it, expect } from 'vitest';
import { getPromptSectionButtons, shouldLiftSectionButtons } from '../PromptInput';
import type { BannerState } from '../WaitingBanner';

// Args: (section, isActive, hasPendingChanges, hasContent).
// isActive = mid-turn OR has active children — collapsed at the call site.
describe('getPromptSectionButtons', () => {
  it('Review threads add Save (Archive comes from WaitingBanner)', () => {
    expect(getPromptSectionButtons('review', false, false, false)).toEqual(['save']);
  });

  // Order is Saved-then-Archive so the toggle stays anchored where it lives in
  // every other state (active, composing — where Archive is suppressed).
  it('Saved threads add Unsave and Archive when idle', () => {
    expect(getPromptSectionButtons('saved', false, false, false)).toEqual(['unsave', 'archive']);
  });

  it('Archive threads add Save', () => {
    expect(getPromptSectionButtons('archive', false, false, false)).toEqual(['save']);
  });

  it('Active threads add Save', () => {
    expect(getPromptSectionButtons('active', true, false, false)).toEqual(['save']);
  });

  it('Active section returns no buttons while composing', () => {
    expect(getPromptSectionButtons('active', true, false, true)).toEqual([]);
  });

  // Save / ✓ Saved stays visible while Apply is pending so the user can park
  // the thread alongside resolving the change. WaitingBanner renders
  // Discard + Apply in the same row, after Save.
  it('Active section keeps Save when Apply is pending', () => {
    expect(getPromptSectionButtons('active', true, true, false)).toEqual(['save']);
  });

  // Saved-section threads stay in `saved` while running (saved wins over
  // running in display_section). The unsave toggle stays so the user can
  // drop a running thread out of Saved and let it route to Active → Review
  // when it idles.
  it('Saved section keeps the unsave toggle while active', () => {
    expect(getPromptSectionButtons('saved', true, false, false)).toEqual(['unsave']);
  });

  // Composing parallels active for the saved section: Send takes Archive's
  // slot but the unsave toggle stays so a Saved thread can be dropped back to
  // regular flow without first sending or canceling.
  it('Saved section keeps the unsave toggle while composing', () => {
    expect(getPromptSectionButtons('saved', false, false, true)).toEqual(['unsave']);
  });

  it('Review section returns no buttons while composing', () => {
    expect(getPromptSectionButtons('review', false, false, true)).toEqual([]);
  });

  it('Archive section returns no buttons while composing', () => {
    expect(getPromptSectionButtons('archive', false, false, true)).toEqual([]);
  });

  // Review/archive while active only matters for the rare race where status
  // flips before display_section recomputes — display_section normally routes
  // running threads to `active`.
  it('Review section returns no buttons while active', () => {
    expect(getPromptSectionButtons('review', true, false, false)).toEqual([]);
  });

  it('Archive section returns no buttons while active', () => {
    expect(getPromptSectionButtons('archive', true, false, false)).toEqual([]);
  });

  // Save / ✓ Saved stays visible while Apply is pending so the user can park
  // the thread without resolving the change first. WaitingBanner renders
  // Discard + Apply in the same row, after the section button. Archive is
  // suppressed on Saved when pending — Discard owns "drop the changes".
  it('Review section keeps Save when Apply is pending', () => {
    expect(getPromptSectionButtons('review', false, true, false)).toEqual(['save']);
  });

  it('Saved section keeps the unsave toggle when Apply is pending', () => {
    expect(getPromptSectionButtons('saved', false, true, false)).toEqual(['unsave']);
  });

  it('Saved section keeps the unsave toggle when Apply is pending while active', () => {
    expect(getPromptSectionButtons('saved', true, true, false)).toEqual(['unsave']);
  });
});

describe('shouldLiftSectionButtons', () => {
  function actionsState(requiresRestart: boolean): BannerState {
    return {
      type: 'actions',
      actions: ['discard', 'apply'],
      threadId: 'tid',
      isArchiving: false,
      requiresRestart,
      incomplete: false,
      pendingChange: null,
      ccDiff: 'hidden',
    };
  }

  it('lifts Save when stacked AND Apply requires restart', () => {
    expect(shouldLiftSectionButtons(true, actionsState(true))).toBe(true);
  });

  it('does not lift Save when stacked but Apply is the short label', () => {
    expect(shouldLiftSectionButtons(true, actionsState(false))).toBe(false);
  });

  it('does not lift Save when not stacked, even with Apply & Restart', () => {
    expect(shouldLiftSectionButtons(false, actionsState(true))).toBe(false);
  });

  it('does not lift Save while applying / discarding (Save sits next to the spinner)', () => {
    expect(shouldLiftSectionButtons(true, { type: 'applying' })).toBe(false);
    expect(shouldLiftSectionButtons(true, { type: 'discarding' })).toBe(false);
  });

  it('does not lift Save when there is no banner (compose state)', () => {
    expect(shouldLiftSectionButtons(true, null)).toBe(false);
  });
});
