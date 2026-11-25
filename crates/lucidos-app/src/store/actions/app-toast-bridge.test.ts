import { describe, it, expect, vi, beforeEach } from 'vitest';

const store = vi.hoisted(() => ({
  showToast: vi.fn(),
  removeToast: vi.fn(),
  dismissToast: vi.fn(),
}));

vi.mock('../store', () => ({
  showToast: store.showToast,
  removeToast: store.removeToast,
  dismissToast: store.dismissToast,
  defaultToastDurationMs: () => 0,
}));

import { handleAppToastMessage } from './app-toast-bridge';

describe('handleAppToastMessage dismiss', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('reports that it owned the dismissal, so the caller stops routing', () => {
    expect(handleAppToastMessage('lucidos:ui:dismissToast', { key: 'x' })).toBe(true);
  });

  it('removes the keyed toast structurally, never through the user-dismiss path', () => {
    handleAppToastMessage('lucidos:ui:dismissToast', { key: 'update-available' });
    expect(store.removeToast).toHaveBeenCalledWith('update-available');
    expect(store.dismissToast).not.toHaveBeenCalled();
  });
});
