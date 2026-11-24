import { describe, it, expect } from 'vitest';
import { credentialRowLabel } from './CredentialItem';

describe('credentialRowLabel', () => {
  it('explains an oauth_client row as the app registration behind a connected account', () => {
    const { title, note } = credentialRowLabel('dropbox', 'oauth_client');
    expect(title).toBe('dropbox');
    expect(note).toContain('App registration');
    expect(note).toContain('dropbox');
    expect(note).toContain('connected account');
  });

  it('explains an email_password row as a mailbox password', () => {
    expect(credentialRowLabel('work', 'email_password')).toEqual({
      title: 'work',
      note: 'Mailbox password',
    });
  });

  it('leaves a free-form service name alone', () => {
    expect(credentialRowLabel('github', 'api_key')).toEqual({ title: 'github', note: null });
    expect(credentialRowLabel('my-oauth-thing', 'bearer')).toEqual({
      title: 'my-oauth-thing',
      note: null,
    });
  });

  it('reads the type, never the shape of the name', () => {
    expect(credentialRowLabel('oauth:leftover', 'api_key').note).toBeNull();
    expect(credentialRowLabel('emailer', 'api_key').note).toBeNull();
    expect(credentialRowLabel('acme', 'oauth_client').note).toContain('App registration');
  });
});
