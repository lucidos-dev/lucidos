import { describe, it, expect, beforeEach } from 'vitest';
import { engineRestarting, toasts, showToast } from '../store';


beforeEach(() => {
  engineRestarting.value = false;
  toasts.value = [];
});

describe('toast suppression while engine is restarting', () => {
  it('shows toasts normally when not restarting', () => {
    showToast('Saved', 'success');
    expect(toasts.value.some(t => t.message === 'Saved')).toBe(true);
  });

  it('suppresses a plain info toast while restarting', () => {
    engineRestarting.value = true;
    showToast('Some status', 'info');
    expect(toasts.value).toHaveLength(0);
  });

  it('suppresses a failure toast (e.g. "Failed to fetch changes") while restarting', () => {
    engineRestarting.value = true;
    showToast('Failed to fetch changes: Failed to fetch', 'error');
    expect(toasts.value).toHaveLength(0);
  });

  it('suppresses the SW "New version available" refresh prompt while restarting', () => {
    engineRestarting.value = true;
    showToast('New version available — refresh to sync', 'info', {
      key: 'update-available',
      action: { label: 'Refresh', onClick: () => {} },
    });
    expect(toasts.value.some(t => t.message === 'New version available — refresh to sync')).toBe(false);
  });

  it('does NOT update an existing keyed toast while restarting', () => {
    showToast('Before', 'info', { key: 'k' });
    engineRestarting.value = true;
    showToast('After', 'info', { key: 'k' });
    const toast = toasts.value.find(t => t.key === 'k');
    expect(toast?.message).toBe('Before');
  });

  it('lets an opted-in status toast through via showWhileUnavailable', () => {
    engineRestarting.value = true;
    showToast('Downloading embedding model', 'info', {
      key: 'model-download',
      spinning: true,
      dismissable: false,
      showWhileUnavailable: true,
    });
    const toast = toasts.value.find(t => t.message === 'Downloading embedding model');
    expect(toast).toBeTruthy();
    expect(toast?.spinning).toBe(true);
  });

  it('resumes showing toasts once the restart clears the flag', () => {
    engineRestarting.value = true;
    showToast('Suppressed', 'error');
    expect(toasts.value).toHaveLength(0);

    engineRestarting.value = false;
    showToast('Engine restarted', 'success', { autoDismissMs: 5_000 });
    expect(toasts.value.some(t => t.message === 'Engine restarted')).toBe(true);
  });
});
