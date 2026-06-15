import { describe, it, expect, beforeAll, beforeEach } from 'vitest';
import { toasts, applyAllInProgress } from '../store';

// Importing the effects module registers the sticky apply-all-batch toast effect
// (and the rest). It must load before we flip `applyAllInProgress`, so the effect
// is subscribed when the signal changes.
beforeAll(async () => {
  await import('../effects');
});

beforeEach(() => {
  applyAllInProgress.value = false;
  toasts.value = [];
});

describe('sticky Apply All batch toast', () => {
  it('raises a non-dismissable spinner toast for the lifetime of the batch', () => {
    expect(toasts.value.some((t) => t.key === 'apply-all-batch')).toBe(false);

    applyAllInProgress.value = true;

    const toast = toasts.value.find((t) => t.key === 'apply-all-batch');
    expect(toast).toBeDefined();
    expect(toast?.message).toBe('Applying changes...');
    expect(toast?.spinning).toBe(true);
    expect(toast?.dismissable).toBe(false);
    // Cancel action lets the user stop the whole batch from the toast.
    expect(toast?.action?.label).toBe('Cancel');
    expect(typeof toast?.action?.onClick).toBe('function');
  });

  it('dismisses the toast when the batch completes', () => {
    applyAllInProgress.value = true;
    expect(toasts.value.some((t) => t.key === 'apply-all-batch')).toBe(true);

    applyAllInProgress.value = false;
    expect(toasts.value.some((t) => t.key === 'apply-all-batch')).toBe(false);
  });
});
