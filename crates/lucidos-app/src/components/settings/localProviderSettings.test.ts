/**
 * Saving a new local base URL moves the saved key with it.
 *
 * The engine sends the `local` key only inside its credential scope. A URL
 * saved on its own would leave local chat keyless, and the server answers 401.
 */
import { describe, it, expect } from 'vitest';
import { localKeyRescope } from './LocalProviderSettings';
import type { CredentialInfo } from '../../store/types';

const key = (base_urls: string[], env_var_name: string | null = null): CredentialInfo => ({
  id: 'cred-1',
  service_name: 'local',
  base_urls,
  auth_type: 'api_key',
  auth_header: 'Authorization',
  created_at: '2026-01-01T00:00:00Z',
  env_var_name,
});

describe('localKeyRescope', () => {
  it('moves the key to a newly saved URL', () => {
    expect(localKeyRescope(key(['http://localhost:1234/v1']), ' http://localhost:1235/v1 ')).toEqual({
      base_urls: ['http://localhost:1235/v1'],
      auth_type: 'api_key',
      auth_header: 'Authorization',
      env_var_name: undefined,
    });
  });

  it('keeps a custom env var name, since an edit replaces every field', () => {
    expect(localKeyRescope(key(['http://a/v1'], 'LOCAL_KEY'), 'http://b/v1')?.env_var_name).toBe('LOCAL_KEY');
  });

  it('does nothing when the key already covers the URL', () => {
    expect(localKeyRescope(key(['http://localhost:1234/v1']), 'http://localhost:1234/v1')).toBeNull();
  });

  it('does nothing with no saved key', () => {
    expect(localKeyRescope(undefined, 'http://localhost:1234/v1')).toBeNull();
  });

  it('leaves the key alone when the field is cleared, since the engine may fall back to an env URL', () => {
    expect(localKeyRescope(key(['http://localhost:1234/v1']), '  ')).toBeNull();
  });
});
