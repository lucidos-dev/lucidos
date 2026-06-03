import { describe, it, expect } from 'vitest';
import { shouldLiftSectionButtons } from '../PromptInput';
import type { BannerState } from '../WaitingBanner';
import type { TaggedAction } from '../../../store/actions/threadActions';

// Save/Unsave selection now derives from resolveThreadActions (the save-category
// action). The lift heuristic keys off the Apply TaggedAction's label, which is
// "Apply & Restart" exactly when a restart is required — single-sourced from the
// availability selector.
describe('shouldLiftSectionButtons', () => {
  function action(kind: TaggedAction['kind'], label: string, category: TaggedAction['category']): TaggedAction {
    return { kind, category, label, invoke: () => {} };
  }
  function actionsState(requiresRestart: boolean): BannerState {
    return {
      type: 'actions',
      actions: [
        action('discard', 'Discard', 'close'),
        action('apply', requiresRestart ? 'Apply & Restart' : 'Apply', 'primary'),
      ],
      threadId: 'tid',
      isArchiving: false,
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
