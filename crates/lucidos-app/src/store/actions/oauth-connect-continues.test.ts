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
import { closeCredentialForm, submitRequestedCredential } from './credentials';
import type { CredentialRequest } from '../types';

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
      ['https://api.dropboxapi.test'],
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(reauthorizeOAuth).toHaveBeenLastCalledWith('dropbox', 'files.content.write', false);
    expect(openUrlOutsideApp).toHaveBeenCalledWith('https://dropbox.test/authorize?x=1');
  });

  it('keeps the sign-in request through the registration form', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', '', true);
    expect(reauthorizeOAuth).toHaveBeenLastCalledWith('dropbox', '', true);

    reauthorizeOAuth.mockResolvedValueOnce(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      ['https://api.dropboxapi.test'],
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(reauthorizeOAuth).toHaveBeenLastCalledWith('dropbox', '', true);
  });

  it('does not fire twice for one press', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');
    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);

    const save = () =>
      submitRequestedCredential(
        { service: 'dropbox' },
        'dropbox',
        ['https://api.dropboxapi.test'],
        'oauth_client',
        '{"client_id":"abc"}',
      );
    await save();
    await save();

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
      ['https://api.dropboxapi.test'],
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(openUrlOutsideApp).not.toHaveBeenCalled();
  });

  it('is not consumed by an unrelated credential saved later', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');

    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'jira' },
      'jira',
      ['https://jira.test'],
      'api_key',
      'secret',
    );

    expect(openUrlOutsideApp).not.toHaveBeenCalled();
  });

  it('abandons the pending connect when the registration form is closed', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(needsCredentials());
    await grantOAuthScope('dropbox', 'files.content.write');

    closeCredentialForm();
    reauthorizeOAuth.mockResolvedValue(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      ['https://api.dropboxapi.test'],
      'oauth_client',
      '{"client_id":"abc"}',
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
      ['https://api.dropboxapi.test'],
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
      ['https://api.dropboxapi.test'],
      'oauth_client',
      '{"client_id":"abc","auth_url":"https://dropbox.test/authorize"}',
    );

    expect(createCredential).not.toHaveBeenCalled();
    expect(updateCredential).toHaveBeenCalledWith('cred-1', expect.objectContaining({
      auth_type: 'oauth_client',
      base_urls: ['https://api.dropboxapi.test'],
    }));
  });

  it('creates when the request names no existing row', async () => {
    reauthorizeOAuth.mockResolvedValueOnce(AUTHORIZES);
    await submitRequestedCredential(
      { service: 'dropbox' },
      'dropbox',
      ['https://api.dropboxapi.test'],
      'oauth_client',
      '{"client_id":"abc"}',
    );

    expect(updateCredential).not.toHaveBeenCalled();
    expect(createCredential).toHaveBeenCalled();
  });
});

describe('a scope widening keeps everything but the scope', () => {
  const widening: CredentialRequest = {
    service: 'github',
    existing_credential_id: 'cred-2',
    base_urls: ['https://api.github.com', 'https://github.com'],
    adding_base_urls: ['https://github.com'],
  };

  it('writes the stored auth header, not the Authorization default', async () => {
    await submitRequestedCredential(
      widening,
      'github',
      widening.base_urls!,
      'api_key',
      'ghp_secret',
      undefined,
      'X-Api-Key',
    );

    expect(createCredential).not.toHaveBeenCalled();
    expect(updateCredential).toHaveBeenCalledWith('cred-2', expect.objectContaining({
      auth_header: 'X-Api-Key',
      auth_type: 'api_key',
      base_urls: ['https://api.github.com', 'https://github.com'],
      auth_value: 'ghp_secret',
    }));
  });

  it('falls back to Authorization when no header is passed', async () => {
    await submitRequestedCredential(
      widening,
      'github',
      widening.base_urls!,
      'api_key',
      'ghp_secret',
    );

    expect(updateCredential).toHaveBeenCalledWith('cred-2', expect.objectContaining({
      auth_header: 'Authorization',
    }));
  });
});
