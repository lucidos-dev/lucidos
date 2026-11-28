import { describe, it, expect, beforeEach, vi } from 'vitest';
import { oauthAccounts, panelOverlay, toasts } from '../store';

const THIS_DEVICE = 'device-aaa';
const OTHER_DEVICE = 'device-bbb';

vi.mock('./devices', () => ({ getDeviceId: () => THIS_DEVICE }));

const platformMocks = vi.hoisted(() => ({ isTauri: true }));
vi.mock('../../utils/platform', () => ({
  isTauri: () => platformMocks.isTauri,
  isIOSPwa: () => false,
  isIOS: () => false,
}));

const focusCallingWindow = vi.hoisted(() => vi.fn());
const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  currentWindowLabel: () => null,
  focusCallingWindow,
  openExternal,
  setTitlebarColor: () => Promise.resolve(),
  windowReadyToShow: () => {},
}));

vi.mock('./credentials', () => ({ openCredentialRequest: vi.fn() }));
const listOAuthAccounts = vi.hoisted(() => vi.fn(() => Promise.resolve({ accounts: [] })));
const reauthorizeOAuth = vi.hoisted(() => vi.fn());
const completeOAuth = vi.hoisted(() => vi.fn());
vi.mock('../../api/client', () => ({
  listOAuthAccounts,
  deleteOAuthAccountApi: vi.fn(),
  reauthorizeOAuth,
  completeOAuth,
}));

const openUrlOutsideApp = vi.hoisted(() => vi.fn());
vi.mock('./artifacts', () => ({ openUrlOutsideApp }));

const {
  handleOAuthAccountConnected,
  presentOAuthAccountConnected,
  openOAuthAuthorizationUrl,
  oauthAuthFlow,
  grantOAuthScope,
} = await import('./oauth');

const submitInteractiveRequest = vi.fn((request, _id?: string) => presentOAuthAccountConnected(request.payload));
vi.mock('../interactiveDeliveryService', () => ({ submitInteractiveRequest: (...args: Parameters<typeof submitInteractiveRequest>) => submitInteractiveRequest(...args) }));

const AUTH_URL = 'https://www.dropbox.com/oauth2/authorize?client_id=k8f2m9qxz1abc4d';

function connected(actorDeviceId: string | null, email: string | null = 'me@example.com') {
  return {
    provider: 'dropbox',
    email,
    actor: actorDeviceId ? { kind: 'device', device_id: actorDeviceId } : null,
  };
}

describe('handleOAuthAccountConnected', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    oauthAccounts.value = { status: 'not-loaded' };
    oauthAuthFlow.value = null;
    toasts.value = [];
    platformMocks.isTauri = true;
    focusCallingWindow.mockClear();
    openUrlOutsideApp.mockClear();
    listOAuthAccounts.mockClear();
    reauthorizeOAuth.mockClear();
    completeOAuth.mockClear();
  });

  it('reloads the accounts list for any device, but only when it is loaded', () => {
    handleOAuthAccountConnected(connected(OTHER_DEVICE));
    expect(listOAuthAccounts).not.toHaveBeenCalled();

    oauthAccounts.value = { status: 'loaded', data: [] };
    handleOAuthAccountConnected(connected(OTHER_DEVICE));
    expect(listOAuthAccounts).toHaveBeenCalledTimes(1);
  });

  it('fronts the window and toasts on the page that started the flow', () => {
    openOAuthAuthorizationUrl(AUTH_URL, 'dropbox');
    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusCallingWindow).toHaveBeenCalledTimes(1);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('dropbox');
    expect(toasts.value[0].message).toContain('me@example.com');
  });

  it('does not front a window whose own flow was superseded by another provider', () => {
    openOAuthAuthorizationUrl('https://accounts.google.com/o/oauth2/auth', 'google');

    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusCallingWindow).not.toHaveBeenCalled();
    expect(oauthAuthFlow.value).toBeNull();
  });

  it('fronts a flow opened without a provider on any completion', () => {
    openOAuthAuthorizationUrl(AUTH_URL);
    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusCallingWindow).toHaveBeenCalledTimes(1);
  });

  it('does not front a window that never opened an authorization page', () => {
    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusCallingWindow).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
  });

  it('leaves a device that did not start the flow alone', () => {
    openOAuthAuthorizationUrl(AUTH_URL);
    handleOAuthAccountConnected(connected(OTHER_DEVICE));
    expect(focusCallingWindow).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  it('does nothing device-scoped when the event carries no actor', () => {
    openOAuthAuthorizationUrl(AUTH_URL);
    handleOAuthAccountConnected(connected(null));
    expect(focusCallingWindow).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  it('does not reach for the native window outside the desktop app', () => {
    platformMocks.isTauri = false;
    openOAuthAuthorizationUrl(AUTH_URL);
    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusCallingWindow).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(1);
  });

  it('omits the email when the provider did not report one', () => {
    handleOAuthAccountConnected(connected(THIS_DEVICE, null));
    expect(toasts.value[0].message).toBe('dropbox connected');
  });

  describe('the authorization page leaves the app', () => {
    it('hands the URL to the outside-the-app opener, never the in-app panel', () => {
      openOAuthAuthorizationUrl(AUTH_URL);
      expect(openUrlOutsideApp).toHaveBeenCalledWith(AUTH_URL);
      expect(panelOverlay.value).toBeNull();
    });

    it('still records the flow marker, which is what fronts this window later', () => {
      openOAuthAuthorizationUrl(AUTH_URL, 'dropbox');
      expect(oauthAuthFlow.value).toEqual({ provider: 'dropbox' });
    });

    it('leaves whatever the panel is showing alone', () => {
      openOAuthAuthorizationUrl(AUTH_URL);
      panelOverlay.value = { type: 'url-preview', url: 'https://example.com/other' };

      handleOAuthAccountConnected(connected(THIS_DEVICE));
      expect(panelOverlay.value).toEqual({
        type: 'url-preview',
        url: 'https://example.com/other',
      });
    });
  });
});

describe('the OAuth delivery boundary', () => {
  it('queues identity and keeps focus independent of toast acceptance', () => {
    oauthAuthFlow.value = null;
    toasts.value = [];
    submitInteractiveRequest.mockImplementationOnce(() => {});
    openOAuthAuthorizationUrl(AUTH_URL, 'dropbox');
    handleOAuthAccountConnected(connected(THIS_DEVICE), 'connected-event');
    expect(submitInteractiveRequest).toHaveBeenLastCalledWith(expect.objectContaining({ kind: 'oauth-connected' }), 'connected-event');
    expect(focusCallingWindow).toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });
});

describe('grantOAuthScope success feedback', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    oauthAccounts.value = { status: 'not-loaded' };
    oauthAuthFlow.value = null;
    toasts.value = [];
    platformMocks.isTauri = false;
    reauthorizeOAuth.mockResolvedValue({ success: true, auth_url: AUTH_URL });
    completeOAuth.mockResolvedValue({ success: true });
    focusCallingWindow.mockClear();
  });

  it('toasts itself when no connected-event reached this device', async () => {
    await grantOAuthScope('dropbox', 'files.content.write');
    expect(toasts.value.map((t) => t.message)).toEqual(['dropbox connected']);
  });

  it('stays quiet when the connected-event already reported it', async () => {
    completeOAuth.mockImplementation(async () => {
      handleOAuthAccountConnected(connected(THIS_DEVICE));
      return { success: true };
    });

    await grantOAuthScope('dropbox', 'files.content.write');
    expect(toasts.value.map((t) => t.message)).toEqual(['dropbox connected (me@example.com)']);
  });

  it('still toasts when the connected-event named a different device', async () => {
    completeOAuth.mockImplementation(async () => {
      handleOAuthAccountConnected(connected(OTHER_DEVICE));
      return { success: true };
    });

    await grantOAuthScope('dropbox', 'files.content.write');
    expect(toasts.value.map((t) => t.message)).toEqual(['dropbox connected']);
  });
});
