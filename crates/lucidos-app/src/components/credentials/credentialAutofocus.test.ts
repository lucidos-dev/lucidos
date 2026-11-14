import { describe, it, expect } from 'vitest';
import { pickCredentialAutofocus } from './credentialAutofocus';

describe('pickCredentialAutofocus', () => {
  it('returns null on mobile for every auth type', () => {
    expect(pickCredentialAutofocus('api_key', true)).toBe(null);
    expect(pickCredentialAutofocus('bearer', true)).toBe(null);
    expect(pickCredentialAutofocus('basic', true)).toBe(null);
    expect(pickCredentialAutofocus('password', true)).toBe(null);
    expect(pickCredentialAutofocus('oauth_client', true)).toBe(null);
    expect(pickCredentialAutofocus('email_password', true)).toBe(null);
  });

  it('routes desktop focus to the right field per auth type', () => {
    expect(pickCredentialAutofocus('api_key', false)).toBe('authValue');
    expect(pickCredentialAutofocus('bearer', false)).toBe('authValue');
    expect(pickCredentialAutofocus('basic', false)).toBe('authValue');
    expect(pickCredentialAutofocus('email_password', false)).toBe('authValue');
    expect(pickCredentialAutofocus('password', false)).toBe('username');
    expect(pickCredentialAutofocus('oauth_client', false)).toBe('clientId');
  });
});
