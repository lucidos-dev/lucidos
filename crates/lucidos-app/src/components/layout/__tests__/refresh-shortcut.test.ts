/** The Refresh shortcut runs the open panel's refresh where the header's
 *  Refresh can, and nowhere else.
 *
 *  Plan: `docs/plans/2026-10-02-shortcut-coverage-and-search-landing.md`. */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { panelOverlay } from '../../../store/store';
import { registerPanelRefresh } from '../../../store/panelRefresh';
import { refreshPanelIfLive } from '../RefreshIndicator';

let unregister: (() => void) | null = null;
const action = vi.fn(() => Promise.resolve());

beforeEach(() => {
  action.mockClear();
  panelOverlay.value = null;
});

afterEach(() => {
  unregister?.();
  unregister = null;
  panelOverlay.value = null;
});

describe('refreshPanelIfLive', () => {
  it('runs the open panel refresh', () => {
    unregister = registerPanelRefresh('apps', action);
    refreshPanelIfLive();
    expect(action).toHaveBeenCalledTimes(1);
  });

  it('does nothing with no refresh registered', () => {
    refreshPanelIfLive();
    expect(action).not.toHaveBeenCalled();
  });

  it('leaves a diff pinned to its change', () => {
    unregister = registerPanelRefresh('diff', action);
    panelOverlay.value = { type: 'file-preview', path: 'repo:lucidos:diff:main...topic:src/a.ts' } as typeof panelOverlay.value;
    refreshPanelIfLive();
    expect(action).not.toHaveBeenCalled();
  });
});
