import { describe, it, expect, beforeAll, beforeEach } from 'vitest';
import { clientRefreshing, noteUpdateBuildId, wasSwUpdateDismissed } from '../../hooks/sw-update';
import { toasts, showToast, preferences } from '../store';

beforeAll(async () => {
  await import('../effects');
});

describe('refreshing spinner toast', () => {
  beforeEach(() => {
    clientRefreshing.value = false;
    toasts.value = [];
    preferences.value = { status: 'loaded', data: {} };
    noteUpdateBuildId('served-new');
    showToast('New version available', 'info', {
      key: 'update-available',
      action: { label: 'Refresh', onClick: () => {} },
    });
  });

  it('replaces the "New version available" toast with the spinner when a client refresh starts', () => {
    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(true);
    expect(toasts.value.some((t) => t.key === 'refreshing')).toBe(false);

    clientRefreshing.value = true;

    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(false);
    const toast = toasts.value.find((t) => t.key === 'refreshing');
    expect(toast).toBeDefined();
    expect(toast?.message).toBe('Refreshing...');
    expect(toast?.spinning).toBe(true);
    expect(toast?.dismissable).toBe(false);
  });

  it('does not record the build as dismissed, so other devices keep their prompt', () => {
    clientRefreshing.value = true;

    expect(wasSwUpdateDismissed('served-new')).toBe(false);
  });
});
