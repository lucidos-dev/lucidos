import { describe, it, expect, beforeEach } from 'vitest';
import { engineRestarting, toasts, showToast } from '../store';

// While the engine is restarting, in-flight read requests fail as the engine
// goes down (a GET already past the awaitEngineReady gate, SSE, health poll).
// showToast suppresses the resulting noise so only the "Restarting engine..."
// status (which opts in via showDuringRestart) is visible — see the central guard
// in store.ts. These pin the screenshot regression: the SW "New version
// available" refresh prompt and the "Failed to fetch changes" error must NOT show
// during a restart. (The UI is no longer deactivated during a restart, but this
// read-noise suppression is unchanged — user-initiated write failures surface
// elsewhere, e.g. a send's inline ResponseFailed, not via this path.)

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
    // Mirrors useStartup's onUpdateFound: the post-restart frontend rebuild
    // activates a new service worker, which would otherwise stack a Refresh
    // toast on top of the "Restarting engine..." status.
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

  it('lets the restart status toast through via showDuringRestart', () => {
    engineRestarting.value = true;
    showToast('Restarting engine...', 'info', {
      key: 'restart-required',
      spinning: true,
      dismissable: false,
      showDuringRestart: true,
    });
    const toast = toasts.value.find(t => t.message === 'Restarting engine...');
    expect(toast).toBeTruthy();
    expect(toast?.spinning).toBe(true);
  });

  it('resumes showing toasts once the restart clears the flag', () => {
    engineRestarting.value = true;
    showToast('Suppressed', 'error');
    expect(toasts.value).toHaveLength(0);

    // connection.ts / UiBlockingOverlay clear the flag before emitting their toasts.
    // The real "Engine restarted" toast carries no action (the refresh prompt is
    // owned solely by the build-id staleness check).
    engineRestarting.value = false;
    showToast('Engine restarted', 'success', { autoDismissMs: 5_000 });
    expect(toasts.value.some(t => t.message === 'Engine restarted')).toBe(true);
  });
});
