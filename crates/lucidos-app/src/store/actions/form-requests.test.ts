import { describe, it, expect, beforeEach, vi } from 'vitest';
import { activeInlineForm, panelOverlay } from '../store';
import type { CredentialRequest } from '../types';
import type { FormRequestEvent } from '../thread-events/thread-event-types';
import type { PendingFormRequest } from '../../api/client/settings';

const listPendingFormRequests = vi.fn<() => Promise<PendingFormRequest[]>>();
const listResolvedFormRequests = vi.fn(async (_ids: string[]) => [] as string[]);
vi.mock('../../api/client/settings', () => ({
  listPendingFormRequests: () => listPendingFormRequests(),
  listResolvedFormRequests: (ids: string[]) => listResolvedFormRequests(ids),
  cancelFormRequest: vi.fn(),
}));

const openCredentialRequest = vi.fn((request: CredentialRequest) => {
  panelOverlay.value = { type: 'form', form: { type: 'credential', request } };
});
vi.mock('./credentials', () => ({ openCredentialRequest }));
vi.mock('./plugin-install', () => ({ openPluginInstallRequest: vi.fn() }));
vi.mock('./plugin-uninstall', () => ({ openPluginUninstallRequest: vi.fn() }));
vi.mock('./email-confirm', () => ({ openEmailConfirmRequest: vi.fn() }));

const THIS_DEVICE = 'device-here';
vi.mock('./devices', () => ({ getDeviceId: () => THIS_DEVICE }));

const presentThreadNavigation = vi.fn();
const handleNavigationRequest = vi.fn();
vi.mock('./navigation-request', () => ({
  presentThreadNavigation,
  handleNavigationRequest,
}));
vi.mock('./thread-label', () => ({ formatThreadLabel: (id: string) => `thread ${id}` }));

const {
  closeResolvedFormRequest,
  openFormRequest,
  presentFormRequest,
  resetOfferedFormRequests,
  syncPendingFormRequests,
} = await import('./form-requests');

const submitInteractiveRequest = vi.fn((request, _id?: string) => {
  presentFormRequest(request.payload.threadId, request.payload.event);
});
const resolveInteractiveForm = vi.fn();
const reconcileInteractiveForms = vi.fn();
const snapshotInteractiveForms = vi.fn(async () => [] as string[]);
vi.mock('../interactiveDeliveryService', () => ({ synchronizeInteractiveForms: (body: () => Promise<void>) => body(), submitInteractiveRequest: (...args: Parameters<typeof submitInteractiveRequest>) => submitInteractiveRequest(...args), resolveInteractiveForm: (id: string) => resolveInteractiveForm(id), reconcileInteractiveForms: (...args: string[][]) => reconcileInteractiveForms(...args), snapshotInteractiveForms: () => snapshotInteractiveForms() }));
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
    actor: { kind: 'device', device_id: deviceId },
  };
}

beforeEach(() => {
  panelOverlay.value = null;
  resetOfferedFormRequests();
  listPendingFormRequests.mockReset();
  listResolvedFormRequests.mockReset().mockResolvedValue([]);
  openCredentialRequest.mockClear();
  presentThreadNavigation.mockClear();
  handleNavigationRequest.mockClear();
  snapshotInteractiveForms.mockReset().mockResolvedValue([]);
  resolveInteractiveForm.mockClear();
  reconcileInteractiveForms.mockClear();
});

describe('a reconnect between emit and render', () => {
  it('still opens the form, from the open requests the next stream open reads', async () => {
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

    closeResolvedFormRequest('req-1', 'completed', { kind: 'device', device_id: 'device-elsewhere' });

    expect(panelOverlay.value).toBeNull();
  });

  it('leaves the answering device to close its own form', () => {
    openFormRequest(THREAD, credentialRequest('req-1'));

    closeResolvedFormRequest('req-1', 'completed', { kind: 'device', device_id: THIS_DEVICE });

    expect(activeInlineForm.value?.type).toBe('credential');
  });

  it('closes here too when a message typed here replaced it', () => {
    openFormRequest(THREAD, credentialRequest('req-1'));

    closeResolvedFormRequest('req-1', 'superseded', { kind: 'device', device_id: THIS_DEVICE });

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

  it('resubmits after local state resets so the journal can deduplicate', async () => {
    const request = credentialRequest('req-1');
    openFormRequest(THREAD, request);
    panelOverlay.value = null;
    resetOfferedFormRequests();
    listPendingFormRequests.mockResolvedValue([pending(request)]);

    await syncPendingFormRequests();

    expect(submitInteractiveRequest).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'form' }), 'form:req-1');
  });
});

describe('the automatic delivery boundary', () => {
  it('queues older forms even when the newest already has a shared receipt', async () => {
    const before = submitInteractiveRequest.mock.calls.length;
    submitInteractiveRequest.mockImplementationOnce(() => {}).mockImplementationOnce(() => {});
    listPendingFormRequests.mockResolvedValue([pending(credentialRequest('older')), pending(credentialRequest('newest'))]);
    await syncPendingFormRequests();
    expect(submitInteractiveRequest.mock.calls.slice(before).map((call) => call[1])).toEqual(['form:newest', 'form:older']);
  });

  it('resolves only journal forms observed before the authoritative pending read', async () => {
    snapshotInteractiveForms.mockResolvedValue(['offline-resolved', 'still-open']);
    listResolvedFormRequests.mockResolvedValue(['offline-resolved']);
    listPendingFormRequests.mockImplementation(async () => {
      snapshotInteractiveForms.mockResolvedValue(['new-live-form']);
      return [pending(credentialRequest('still-open'))];
    });
    await syncPendingFormRequests();
    expect(reconcileInteractiveForms.mock.calls).toEqual([[['offline-resolved', 'still-open'], ['still-open'], ['offline-resolved']]]);
  });

  it('keeps archived unresolved forms eligible when their thread returns', async () => {
    snapshotInteractiveForms.mockResolvedValue(['archived']);
    listPendingFormRequests.mockResolvedValue([]);
    await syncPendingFormRequests();
    expect(listResolvedFormRequests).toHaveBeenCalledWith(['archived']);
    expect(reconcileInteractiveForms).toHaveBeenLastCalledWith(['archived'], [], []);
    listPendingFormRequests.mockResolvedValue([pending(credentialRequest('archived'))]);
    await syncPendingFormRequests();
    expect(submitInteractiveRequest).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'form' }), 'form:archived');
  });

  it('queues the same identity from live and pending sync without electing locally', async () => {
    submitInteractiveRequest.mockImplementationOnce(() => {});
    openFormRequest(THREAD, credentialRequest('queued'));
    expect(openCredentialRequest).not.toHaveBeenCalled();
    submitInteractiveRequest.mockImplementationOnce(() => {});
    listPendingFormRequests.mockResolvedValue([pending(credentialRequest('queued'))]);
    await syncPendingFormRequests();
    expect(submitInteractiveRequest.mock.calls.slice(-2).map((call) => call[1])).toEqual(['form:queued', 'form:queued']);
  });
  it('records resolution even when the answering device keeps its form', () => {
    closeResolvedFormRequest('resolved', 'completed', { kind: 'device', device_id: THIS_DEVICE });
    expect(resolveInteractiveForm).toHaveBeenCalledWith('resolved');
  });
});

describe('an authorization page', () => {
  it('is offered only on the device the engine sent it to', async () => {
    listPendingFormRequests.mockResolvedValue([pending(authorizationRequest('auth-1', 'device-elsewhere'))]);

    await syncPendingFormRequests();

    expect(presentThreadNavigation).not.toHaveBeenCalled();
    expect(handleNavigationRequest).not.toHaveBeenCalled();
  });

  it('routes like a navigation on its own device', async () => {
    const request = authorizationRequest('auth-1', THIS_DEVICE);
    listPendingFormRequests.mockResolvedValue([pending(request)]);

    await syncPendingFormRequests();

    expect(presentThreadNavigation).toHaveBeenCalledWith(
      expect.objectContaining({ purpose: 'oauth' }),
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
