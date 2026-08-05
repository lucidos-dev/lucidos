/**
 * `handleOAuthAccountConnected`: what happens the moment an authorization the
 * user completed in a BROWSER lands back in the engine.
 *
 * The device scoping is the whole point and is what these tests exist to pin.
 * The engine stamps the flow's initiating device onto `OAuthAccountConnected`
 * (see `prepare_oauth_flow`), and only that device may be fronted. An account
 * connected from a phone must not yank a desktop window forward, and an
 * engine-internal reconnect (no actor) belongs to nobody.
 */
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

const focusMainWindow = vi.hoisted(() => vi.fn());
const openExternal = vi.hoisted(() => vi.fn(() => Promise.resolve()));
vi.mock('../../utils/tauri', () => ({
  focusMainWindow,
  openExternal,
  setTitlebarColor: () => Promise.resolve(),
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

const openUrl = vi.hoisted(() => vi.fn());
vi.mock('./artifacts', () => ({ openUrl }));

const {
  handleOAuthAccountConnected,
  openOAuthAuthorizationUrl,
  oauthAuthPanelUrl,
  grantOAuthScope,
} = await import('./oauth');

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
    oauthAuthPanelUrl.value = null;
    toasts.value = [];
    platformMocks.isTauri = true;
    focusMainWindow.mockClear();
    openUrl.mockClear();
    listOAuthAccounts.mockClear();
    reauthorizeOAuth.mockClear();
    completeOAuth.mockClear();
  });

  // The device-scoped half must not eat the accounts reload, which every device
  // needs. Guarded on `loaded` so a page that never opened Settings doesn't
  // fetch a list nothing is rendering.
  it('reloads the accounts list for any device, but only when it is loaded', () => {
    handleOAuthAccountConnected(connected(OTHER_DEVICE));
    expect(listOAuthAccounts).not.toHaveBeenCalled();

    oauthAccounts.value = { status: 'loaded', data: [] };
    handleOAuthAccountConnected(connected(OTHER_DEVICE));
    expect(listOAuthAccounts).toHaveBeenCalledTimes(1);
  });

  it('fronts the window and toasts on the device that started the flow', () => {
    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusMainWindow).toHaveBeenCalledTimes(1);
    expect(toasts.value).toHaveLength(1);
    expect(toasts.value[0].message).toContain('dropbox');
    expect(toasts.value[0].message).toContain('me@example.com');
  });

  // The regression the device actor exists to prevent.
  it('leaves a device that did not start the flow alone', () => {
    handleOAuthAccountConnected(connected(OTHER_DEVICE));
    expect(focusMainWindow).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  // An engine-internal reconnect has no initiating device, so nobody is fronted.
  it('does nothing device-scoped when the event carries no actor', () => {
    handleOAuthAccountConnected(connected(null));
    expect(focusMainWindow).not.toHaveBeenCalled();
    expect(toasts.value).toHaveLength(0);
  });

  it('does not reach for the native window outside the desktop app', () => {
    platformMocks.isTauri = false;
    handleOAuthAccountConnected(connected(THIS_DEVICE));
    expect(focusMainWindow).not.toHaveBeenCalled();
    // The toast still fires: a browser user wants to know it landed.
    expect(toasts.value).toHaveLength(1);
  });

  // A provider with no userinfo endpoint (Dropbox, before the POST support)
  // reports no email. The toast must not render "connected (null)".
  it('omits the email when the provider did not report one', () => {
    handleOAuthAccountConnected(connected(THIS_DEVICE, null));
    expect(toasts.value[0].message).toBe('dropbox connected');
  });

  describe('the in-app browser panel', () => {
    it('closes the panel showing the authorization page it opened', () => {
      openOAuthAuthorizationUrl(AUTH_URL);
      expect(openUrl).toHaveBeenCalledWith(AUTH_URL);
      // openUrl is mocked, so stand the panel up the way the real one would.
      panelOverlay.value = { type: 'url-preview', url: AUTH_URL };

      handleOAuthAccountConnected(connected(THIS_DEVICE));
      expect(panelOverlay.value).toBeNull();
      expect(oauthAuthPanelUrl.value).toBeNull();
    });

    // Matching on the URL rather than a "flow in flight" flag is what makes
    // this safe: an abandoned flow's stale URL can only ever match the
    // authorization page, never whatever the user opened afterwards.
    it('leaves an unrelated page open', () => {
      openOAuthAuthorizationUrl(AUTH_URL);
      panelOverlay.value = { type: 'url-preview', url: 'https://example.com/other' };

      handleOAuthAccountConnected(connected(THIS_DEVICE));
      expect(panelOverlay.value).toEqual({
        type: 'url-preview',
        url: 'https://example.com/other',
      });
    });

    it('leaves a non-url overlay alone', () => {
      openOAuthAuthorizationUrl(AUTH_URL);
      panelOverlay.value = { type: 'app-ui', app: { id: 'habit-tracker' } } as never;

      handleOAuthAccountConnected(connected(THIS_DEVICE));
      expect(panelOverlay.value).not.toBeNull();
    });
  });
});

/**
 * A user who CLICKED Connect must get feedback. `handleOAuthAccountConnected`
 * normally owns the success toast (it knows the email and also fronts the
 * window), but it is device-scoped and silent when the actor isn't this device
 * or the SSE never arrived. Without a fallback the button would just stop
 * spinning, which is what the first cut of this change shipped.
 */
describe('grantOAuthScope success feedback', () => {
  beforeEach(() => {
    panelOverlay.value = null;
    oauthAccounts.value = { status: 'not-loaded' };
    oauthAuthPanelUrl.value = null;
    toasts.value = [];
    platformMocks.isTauri = false;
    reauthorizeOAuth.mockResolvedValue({ success: true, auth_url: AUTH_URL });
    completeOAuth.mockResolvedValue({ success: true });
    focusMainWindow.mockClear();
  });

  it('toasts itself when no connected-event reached this device', async () => {
    await grantOAuthScope('dropbox', 'files.content.write');
    expect(toasts.value.map((t) => t.message)).toEqual(['dropbox connected']);
  });

  // The normal case: the SSE lands first (the engine emits inside its own write
  // path, before the result the complete call awaits), so the richer toast has
  // already fired and this must not stack a second, plainer one on top.
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
