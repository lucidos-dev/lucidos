import { describe, it, expect } from 'vitest';
import { findProviderCredential } from './providerCredential';
import type { AuthType, CredentialInfo, Loadable } from '../../store/types';

function cred(service_name: string, auth_type: AuthType, id: string): CredentialInfo {
  return {
    id,
    service_name,
    base_urls: ['https://api.example.test'],
    auth_type,
    auth_header: 'Authorization',
    created_at: '2026-08-05T00:00:00Z',
  };
}

function loaded(data: CredentialInfo[]): Loadable<CredentialInfo[]> {
  return { status: 'loaded', data };
}

describe('findProviderCredential', () => {
  it('returns the provider key', () => {
    const found = findProviderCredential(loaded([cred('openai', 'api_key', 'key-id')]), 'openai');
    expect(found?.id).toBe('key-id');
  });

  /**
   * The bug this exists for. Since `auth_type` became the discriminator, an
   * OAuth app registration may share a provider's name. Matching on the name
   * alone could bind the block to the registration: it would report the key as
   * configured off a row the engine never reads for this purpose, and Remove
   * would delete the OAuth client, breaking connected-account refresh while
   * leaving the real key in place.
   */
  it('ignores an OAuth registration sharing the provider name', () => {
    const found = findProviderCredential(
      loaded([cred('openai', 'oauth_client', 'oauth-id'), cred('openai', 'api_key', 'key-id')]),
      'openai'
    );
    expect(found?.id).toBe('key-id');
  });

  // Order must not decide it: the list is sorted by service_name, so a tie
  // between the two rows is arbitrary.
  it('ignores it whichever way the list is ordered', () => {
    const found = findProviderCredential(
      loaded([cred('openai', 'api_key', 'key-id'), cred('openai', 'oauth_client', 'oauth-id')]),
      'openai'
    );
    expect(found?.id).toBe('key-id');
  });

  it('finds nothing when only the OAuth registration exists', () => {
    const found = findProviderCredential(
      loaded([cred('openai', 'oauth_client', 'oauth-id')]),
      'openai'
    );
    expect(found).toBeUndefined();
  });

  it('finds nothing before the credentials have loaded', () => {
    expect(findProviderCredential({ status: 'loading' }, 'openai')).toBeUndefined();
    expect(findProviderCredential({ status: 'not-loaded' }, 'openai')).toBeUndefined();
    expect(
      findProviderCredential({ status: 'failed', error: 'boom' }, 'openai')
    ).toBeUndefined();
  });
});
