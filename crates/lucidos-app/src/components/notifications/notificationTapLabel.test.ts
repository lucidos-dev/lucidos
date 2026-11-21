import { describe, it, expect } from 'vitest';
import type { NavigateTarget } from '@lucidos/sdk';
import { navigateTapLabel } from './notificationTapLabel';

describe('navigateTapLabel', () => {
  it('labels the Changes panel for the "ready to apply" trigger push', () => {
    expect(navigateTapLabel({ target: 'changes' })).toBe('View changes');
  });

  it('gives every known navigate target a distinct, non-empty label', () => {
    const targets: NavigateTarget[] = [
      'files',
      'apps',
      'app-store',
      'triggers',
      'thread-queue',
      'changes',
      'notifications',
      'settings',
      'app',
      'file',
      'trigger',
      'thread',
      'new-app',
      'new-trigger',
      'new-chat',
      'url',
    ];
    const labels = targets.map((target) => navigateTapLabel({ target }));
    for (const label of labels) expect(label.length).toBeGreaterThan(0);
    // No two targets collapse to the same button text.
    expect(new Set(labels).size).toBe(targets.length);
  });

  it('falls back to "Open" for an unrecognized target (post-build schema drift)', () => {
    // validateTap casts the wire `to` without constraining `target`, so a
    // target the engine added after this build can reach here at runtime.
    expect(navigateTapLabel({ target: 'space-elevator' as NavigateTarget })).toBe('Open');
  });
});
