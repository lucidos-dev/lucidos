import { describe, it, expect, beforeAll } from 'vitest';
import { clientRefreshing } from '../../hooks/sw-update';
import { toasts } from '../store';

// Importing the effects module registers the refreshing-toast effect (and the
// rest). It must load before we flip `clientRefreshing`, so the effect is
// subscribed when the signal changes.
beforeAll(async () => {
  await import('../effects');
});

describe('refreshing spinner toast', () => {
  it('raises a non-dismissable spinner toast when a client refresh starts', () => {
    // Registration runs the effect once with clientRefreshing=false → no toast.
    expect(toasts.value.some((t) => t.key === 'refreshing')).toBe(false);

    clientRefreshing.value = true;

    const toast = toasts.value.find((t) => t.key === 'refreshing');
    expect(toast).toBeDefined();
    expect(toast?.message).toBe('Refreshing...');
    expect(toast?.spinning).toBe(true);
    expect(toast?.dismissable).toBe(false);
  });
});
