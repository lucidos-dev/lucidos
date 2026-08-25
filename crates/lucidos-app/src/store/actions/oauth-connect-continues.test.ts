/**
 * Pressing Connect once must be enough.
 *
 * The engine answers "no OAuth client yet" (or "the one you have cannot drive a
 * flow") with a credential request, so `grantOAuthScope` opens the registration
 * form and returns false. Nothing used to survive that return: saving the
 * credential closed the form, reloaded the list, and stopped. No one told the
 * user to press Connect again, and no one did it for them.
 *
 * A repair carries the id of the row it must update, too. Creating instead would
 * make a second `oauth_client` for one provider, and a name plus an auth type is
 * the credential's identity, so that pair is a duplicate.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const reauthorizeOAuth = vi.fn();
const completeOAuth = vi.fn();
const listOAuthAccounts = vi.fn(async () => ({ accounts: [] }));
type SaveResult = { success: boolean; error?: string };
const createCredential = vi.fn(async (..._a: unknown[]): Promise<SaveResult> => ({ success: true }));
const updateCredential = vi.fn(async (..._a: unknown[]): Promise<SaveResult> => ({ success: true }));
const listCredentials = vi.fn(async () => ({ credentials: [] }));
const openUrlOutsideApp = vi.fn();

vi.mock('../../api/client', () => ({
  reauthorizeOAuth: (...a: unknown[]) => reauthorizeOAuth(...a),
  completeOAuth: (...a: unknown[]) => completeOAuth(...a),
  listOAuthAccounts: () => listOAuthAccounts(),
  listKnownOAuthProviders: async () => ({ providers: [], default_redirect_uri: '' }),
  deleteOAuthAccountApi: vi.fn(),
  createCredential: (...a: unknown[]) => createCredential(...a),
  updateCredential: (...a: unknown[]) => updateCredential(...a),
  deleteCredentialApi: vi.fn(),
  listCredentials: () => listCredentials(),
}));
vi.mock('./artifacts', () => ({ openUrlOutsideApp: (...a: unknown[]) => openUrlOutsideApp(...a) }));
vi.mock('./devices', () => ({ getDeviceId: () => 'device-1' }));
vi.mock('../../utils/platform', () => ({ isIOSPwa: () => false, isTauri: () => false }));
vi.mock('../../utils/tauri', () => ({ focusCallingWindow: vi.fn() }));
vi.mock('./menu', () => ({ landOnAccountsWithOverlay: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));

import { grantOAuthScope, cancelPendingOAuthConnect } from './oauth';
import { submitRequestedCredential } from './credentials';
import type { CredentialRequest } from '../types';

/** The engine's answer when the registration is missing or incomplete. */
function needsCredentials(over: Partial<CredentialRequest> = {}) {
  return {
    success: false,
    credential_request: { service: 'dropbox', auth_type: 'oauth_client', ...over },
  };
}

const AUTHORIZES = { success: true, auth_url: 'https://dropbox.test/authorize?x=1' };

beforeEach(() => {
  vi.clearAllMocks();
  cancelPendingOAuthConnect();
  completeOAuth.mockResolvedValue({ success: true });
});

describe('a saved registration continues the connection it was blocking', () => {
  it('runs the authorization the user actually pressed Connect for', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    expect(await grantOAuthScope('dropbox', 'files.content.write')).toBe(false);
    expect(openUrlOutsideApp).not.toHaveBeenCalled();

    reauthorizeOAuth.mockResolvedValueOnce(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      'https://api.dropboxapi.test',
      'oauth_client',
      '{"client_id":"abc"}',
    );

    // The same provider and the same scopes: a continuation that dropped the
    // purpose would connect a bare sign-in and send the user back to Backup.
    expect(reauthorizeOAuth).toHaveBeenLastCalledWith('dropbox', 'files.content.write');
    expect(openUrlOutsideApp).toHaveBeenCalledWith('https://dropbox.test/authorize?x=1');
  });

  it('does not fire twice for one press', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');
    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);

    const save = () =>
      submitRequestedCredential(
        { service: 'dropbox' },
        'dropbox',
        'https://api.dropboxapi.test',
        'oauth_client',
        '{"client_id":"abc"}',
      );
    await save();
    await save();

    // One queued continuation, consumed once. Re-running would race the
    // engine's single live authorization and open a second browser tab.
    expect(openUrlOutsideApp).toHaveBeenCalledTimes(1);
  });

  it('starts nothing when the user dismisses the form', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');

    cancelPendingOAuthConnect();
    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      'https://api.dropboxapi.test',
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(openUrlOutsideApp).not.toHaveBeenCalled();
  });

  it('is not consumed by an unrelated credential saved later', async () => {
    // Navigating away from the form does not dismiss it, so a queued
    // continuation can outlive the screen that created it. Opening a browser for
    // a provider the user abandoned is worse than doing nothing.
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');

    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'jira' },
      'jira',
      'https://jira.test',
      'api_key',
      'secret',
    );

    expect(openUrlOutsideApp).not.toHaveBeenCalled();
  });

  it('does not continue when the save failed', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');

    createCredential.mockResolvedValueOnce({ success: false, error: 'nope' });
    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      'https://api.dropboxapi.test',
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(openUrlOutsideApp).not.toHaveBeenCalled();
  });
});

describe('a repair updates the existing registration', () => {
  it('never creates a second OAuth Client for one provider', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox', existing_credential_id: 'cred-1', missing: ['auth_url'] },
      'dropbox',
      'https://api.dropboxapi.test',
      'oauth_client',
      '{"client_id":"abc","auth_url":"https://dropbox.test/authorize"}',
    );

    expect(createCredential).not.toHaveBeenCalled();
    expect(updateCredential).toHaveBeenCalledWith('cred-1', expect.objectContaining({
      auth_type: 'oauth_client',
      base_url: 'https://api.dropboxapi.test',
    }));
  });

  it('creates when the request names no existing row', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      'https://api.dropboxapi.test',
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(updateCredential).not.toHaveBeenCalled();
    expect(createCredential).toHaveBeenCalled();
  });
});
