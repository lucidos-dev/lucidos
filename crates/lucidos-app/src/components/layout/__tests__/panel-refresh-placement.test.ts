/** Where the content header draws the open panel's Refresh. Pinned as pure
 *  decisions rather than a mounted header, for the reason app-popout-action's
 *  test gives: collapse decides where the action lands, not whether it exists. */
import { describe, it, expect } from 'vitest';
import type { App } from '../../../store/types';
import { mobileRefreshActionShown } from '../ContentHeaderActions';
import { panelRefreshLive, refreshPinnedToDiff } from '../RefreshIndicator';

const app: App = { id: 'habit-tracker', name: 'Habit Tracker', description: '', reveal: 'on-load', kind: 'app', reusable: false };
const diff = { type: 'file-preview' as const, path: 'repo:r1:diff#c1:src/main.rs' };

describe('panelRefreshLive', () => {
  it('is live for any panel with a refresh', () => {
    expect(panelRefreshLive(null, true)).toBe(true);
  });

  it('is never live without a registered refresh', () => {
    expect(panelRefreshLive(null, false)).toBe(false);
  });

  // The preview drops its registration after the render that switched to the
  // diff. Drawing a live Refresh then as well claimed the `refresh` key twice.
  it('is never live over a diff, even while the file preview is still registered', () => {
    expect(refreshPinnedToDiff(diff)).toBe(true);
    expect(panelRefreshLive(diff, true)).toBe(false);
  });

  it('pins only a diff, not a plain file preview', () => {
    expect(refreshPinnedToDiff({ type: 'file-preview', path: 'artifacts/report.md' })).toBe(false);
  });
});

describe('mobileRefreshActionShown', () => {
  it('leaves a phone to the pull, so a single Search keeps its own icon', () => {
    expect(mobileRefreshActionShown(null, true)).toBe(false);
  });

  it('shows where Refresh predates the pull, an app or a file preview', () => {
    expect(mobileRefreshActionShown({ type: 'app-ui', app }, true)).toBe(true);
    expect(mobileRefreshActionShown({ type: 'file-preview', path: 'artifacts/report.md' }, true)).toBe(true);
  });

  it('never shows a live Refresh without a registered refresh, or over a diff', () => {
    expect(mobileRefreshActionShown({ type: 'app-ui', app }, false)).toBe(false);
    expect(mobileRefreshActionShown(diff, true)).toBe(false);
  });
});
