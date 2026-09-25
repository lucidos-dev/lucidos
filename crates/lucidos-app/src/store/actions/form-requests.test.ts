/**
 * A *form request* survives a lost stream frame. The engine persists it, and
 * the client reads the open ones on every stream open (engine
 * `engine/form_requests.rs`, plan
 * `docs/plans/2026-09-24-form-requests-survive-a-reconnect.md`).
 *
 * The reported bug: on a swapping Mac the stream reconnected constantly, the
 * one frame carrying a credential request was dropped, and no form ever
 * appeared.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { activeInlineForm, panelOverlay } from '../store';
import type { CredentialRequest } from '../types';
import type { FormRequestEvent } from '../thread-events/thread-event-types';
import type { PendingFormRequest } from '../../api/client/settings';

const listPendingFormRequests = vi.fn<() => Promise<PendingFormRequest[]>>();
vi.mock('../../api/client/settings', () => ({
  listPendingFormRequests: () => listPendingFormRequests(),
  cancelFormRequest: vi.fn(),
}));

// Each opener stands in for what the real one leaves behind: the form on the
// panel overlay. The real ones also navigate, which is not under test here.
const openCredentialRequest = vi.fn((request: CredentialRequest) => {
  panelOverlay.value = { type: 'form', form: { type: 'credential', request } };
});
vi.mock('./credentials', () => ({ openCredentialRequest }));
vi.mock('./plugin-install', () => ({ openPluginInstallRequest: vi.fn() }));
vi.mock('./plugin-uninstall', () => ({ openPluginUninstallRequest: vi.fn() }));
vi.mock('./email-confirm', () => ({ openEmailConfirmRequest: vi.fn() }));

const THIS_DEVICE = 'device-here';
vi.mock('./devices', () => ({ getDeviceId: () => THIS_DEVICE }));

const routeThreadNavigation = vi.fn();
const handleNavigationRequest = vi.fn();
vi.mock('./navigation-request', () => ({
  routeThreadNavigation,
  handleNavigationRequest,
  navigationIsForThisDevice: (actor?: { kind?: string; device_id?: string }) =>
    actor?.kind !== 'device' || actor.device_id === THIS_DEVICE,
}));
vi.mock('./thread-label', () => ({ formatThreadLabel: (id: string) => `thread ${id}` }));

const {
  closeResolvedFormRequest,
  openFormRequest,
  resetOfferedFormRequests,
  syncPendingFormRequests,
} = await import('./form-requests');

const THREAD = 'thread-1';

function credentialRequest(requestId: string, service = 'weather'): FormRequestEvent {
  return {
    type: 'CredentialRequested',
    request_id: requestId,
    payload: JSON.stringify({ service, prompt: 'Paste your API key.', auth_type: 'api_key' }),
  };
}

function pending(event: FormRequestEvent): PendingFormRequest {
  return { thread_id: THREAD, request_id: event.request_id, event };
}

function authorizationRequest(requestId: string, deviceId: string): FormRequestEvent {
  return {
    type: 'OAuthAuthorizationRequested',
    request_id: requestId,
    payload: JSON.stringify({ target: 'url', url: 'https://auth.example.com/authorize', purpose: 'oauth' }),
    actor: { kind: 'device', device_id: deviceId, label: 'My MacBook' },
  };
}

beforeEach(() => {
  panelOverlay.value = null;
  resetOfferedFormRequests();
  listPendingFormRequests.mockReset();
  openCredentialRequest.mockClear();
  routeThreadNavigation.mockClear();
  handleNavigationRequest.mockClear();
});

describe('a reconnect between emit and render', () => {
  it('still opens the form, from the open requests the next stream open reads', async () => {
    // The live frame never reached this page. The next open finds it.
    const request = credentialRequest('req-1');
    listPendingFormRequests.mockResolvedValue([pending(request)]);

    await syncPendingFormRequests();

    expect(openCredentialRequest).toHaveBeenCalledTimes(1);
    const form = activeInlineForm.value;
    expect(form?.type).toBe('credential');
    expect(form?.type === 'credential' && form.request?.form_request_id).toBe('req-1');
    expect(form?.type === 'credential' && form.request?.service).toBe('weather');
  });

  it('offers the newest open request when several are waiting', async () => {
    listPendingFormRequests.mockResolvedValue([
      pending(credentialRequest('older', 'maps')),
      pending(credentialRequest('newer', 'weather')),
    ]);

    await syncPendingFormRequests();

    expect(openCredentialRequest).toHaveBeenCalledTimes(1);
    expect(openCredentialRequest.mock.calls[0][0].form_request_id).toBe('newer');
  });

  it('never replaces a form the user already has open', async () => {
    panelOverlay.value = { type: 'form', form: { type: 'credential' } };
    listPendingFormRequests.mockResolvedValue([pending(credentialRequest('req-1'))]);

    await syncPendingFormRequests();

    expect(openCredentialRequest).not.toHaveBeenCalled();
  });

  it('shrugs off a failed read, since the next open retries it', async () => {
    listPendingFormRequests.mockRejectedValue(new Error('offline'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});

    await syncPendingFormRequests();

    expect(openCredentialRequest).not.toHaveBeenCalled();
    warn.mockRestore();
  });
});

describe('an answered request', () => {
  it('does not reappear: the engine no longer lists it, so nothing opens', async () => {
    listPendingFormRequests.mockResolvedValue([]);

    await syncPendingFormRequests();

    expect(openCredentialRequest).not.toHaveBeenCalled();
    expect(activeInlineForm.value).toBeNull();
  });

  it('closes on every other device when one device answers it', () => {
    openFormRequest(THREAD, credentialRequest('req-1'));
    expect(activeInlineForm.value?.type).toBe('credential');

    closeResolvedFormRequest('req-1', 'completed', { kind: 'device', device_id: 'device-elsewhere', label: 'My iPhone' });

    expect(panelOverlay.value).toBeNull();
  });

  it('leaves the answering device to close its own form', () => {
    openFormRequest(THREAD, credentialRequest('req-1'));

    closeResolvedFormRequest('req-1', 'completed', { kind: 'device', device_id: THIS_DEVICE, label: 'My MacBook' });

    expect(activeInlineForm.value?.type).toBe('credential');
  });

  it('closes here too when a message typed here replaced it', () => {
    openFormRequest(THREAD, credentialRequest('req-1'));

    closeResolvedFormRequest('req-1', 'superseded', { kind: 'device', device_id: THIS_DEVICE, label: 'My MacBook' });

    expect(panelOverlay.value).toBeNull();
  });

  it('leaves a form showing a different request alone', () => {
    openFormRequest(THREAD, credentialRequest('req-2'));

    closeResolvedFormRequest('req-1', 'completed', null);

    expect(activeInlineForm.value?.type).toBe('credential');
  });
});

describe('an open request the user walked away from', () => {
  it('is not re-popped by a refocus, but stays one Open away', async () => {
    const request = credentialRequest('req-1');
    openFormRequest(THREAD, request);
    panelOverlay.value = null;
    listPendingFormRequests.mockResolvedValue([pending(request)]);

    await syncPendingFormRequests();
    expect(openCredentialRequest).toHaveBeenCalledTimes(1);

    openFormRequest(THREAD, request, { byUser: true });
    expect(openCredentialRequest).toHaveBeenCalledTimes(2);
  });

  it('is offered again after a reload', async () => {
    const request = credentialRequest('req-1');
    openFormRequest(THREAD, request);
    panelOverlay.value = null;
    resetOfferedFormRequests();
    listPendingFormRequests.mockResolvedValue([pending(request)]);

    await syncPendingFormRequests();

    expect(openCredentialRequest).toHaveBeenCalledTimes(2);
  });
});

describe('an authorization page', () => {
  it('is offered only on the device the engine sent it to', async () => {
    listPendingFormRequests.mockResolvedValue([pending(authorizationRequest('auth-1', 'device-elsewhere'))]);

    await syncPendingFormRequests();

    expect(routeThreadNavigation).not.toHaveBeenCalled();
    expect(handleNavigationRequest).not.toHaveBeenCalled();
  });

  it('routes like a navigation on its own device', async () => {
    const request = authorizationRequest('auth-1', THIS_DEVICE);
    listPendingFormRequests.mockResolvedValue([pending(request)]);

    await syncPendingFormRequests();

    expect(routeThreadNavigation).toHaveBeenCalledWith(
      expect.objectContaining({ purpose: 'oauth' }),
      request.actor,
      THREAD,
    );
  });

  it('opens here when the user presses Open, whichever device it was sent to', () => {
    openFormRequest(THREAD, authorizationRequest('auth-1', 'device-elsewhere'), { byUser: true });

    expect(handleNavigationRequest).toHaveBeenCalledWith(
      expect.objectContaining({ purpose: 'oauth' }),
      { source: `thread ${THREAD}` },
    );
  });
});
