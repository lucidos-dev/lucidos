import { describe, it, expect, beforeAll } from 'vitest';
import { clientRefreshing } from '../../hooks/sw-update';
import { toasts, showToast } from '../store';

// Importing the effects module registers the refreshing-toast effect (and the
// rest). It must load before we flip `clientRefreshing`, so the effect is
// subscribed when the signal changes.
beforeAll(async () => {
  await import('../effects');
});

describe('refreshing spinner toast', () => {
  it('replaces the "New version available" toast with the spinner when a client refresh starts', () => {
    // Reset and stage the "New version available" prompt as if the build-id check
    // had surfaced it (surfaceUpdateToast).
    clientRefreshing.value = false;
    toasts.value = [];
    showToast('New version available', 'info', {
      key: 'update-available',
      action: { label: 'Refresh', onClick: () => {} },
    });
    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(true);
    expect(toasts.value.some((t) => t.key === 'refreshing')).toBe(false);

    clientRefreshing.value = true;

    // The update prompt is gone (replaced, not stacked) and the spinner is up.
    expect(toasts.value.some((t) => t.key === 'update-available')).toBe(false);
    const toast = toasts.value.find((t) => t.key === 'refreshing');
    expect(toast).toBeDefined();
    expect(toast?.message).toBe('Refreshing...');
    expect(toast?.spinning).toBe(true);
    expect(toast?.dismissable).toBe(false);
  });
});
