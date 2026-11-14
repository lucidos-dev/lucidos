import { describe, it, expect } from 'vitest';
import { credentialRowLabel } from './CredentialItem';

/**
 * The Credentials list mixes free-form credentials with two engine-owned kinds,
 * and without help every row reads the same. A user who had just connected
 * Dropbox saw the app registration sitting next to a stray plain credential of
 * the same name and could tell neither which was which nor what either was for
 * (2026-08-05).
 *
 * The label keys on `auth_type`, not on how the name is spelled. Names used to
 * carry an `oauth:` / `email:` prefix saying the same thing the type already
 * said, and the two could disagree; the storage layer dropped the prefixes, so
 * reading them here would be reading a fact that no longer exists.
 */
describe('credentialRowLabel', () => {
  it('explains an oauth_client row as the app registration behind a connected account', () => {
    const { title, note } = credentialRowLabel('dropbox', 'oauth_client');
    expect(title).toBe('dropbox');
    expect(note).toContain('App registration');
    expect(note).toContain('dropbox');
    // "account" is the word the other section is now called, so the two read as
    // a pair rather than as two unrelated Dropbox entries.
    expect(note).toContain('connected account');
  });

  it('explains an email_password row as a mailbox password', () => {
    expect(credentialRowLabel('work', 'email_password')).toEqual({
      title: 'work',
      note: 'Mailbox password',
    });
  });

  // An ordinary credential is unchanged: no invented note, and the name the user
  // chose is shown verbatim.
  it('leaves a free-form service name alone', () => {
    expect(credentialRowLabel('github', 'api_key')).toEqual({ title: 'github', note: null });
    expect(credentialRowLabel('my-oauth-thing', 'bearer')).toEqual({
      title: 'my-oauth-thing',
      note: null,
    });
  });

  // The drift this rewrite removes, pinned from both directions. A name that
  // merely LOOKS namespaced gets no note, and a genuinely typed row gets one
  // whatever its name looks like. Under the old name-sniffing label the first
  // case was annotated as an app registration and the second was not.
  it('reads the type, never the shape of the name', () => {
    expect(credentialRowLabel('oauth:leftover', 'api_key').note).toBeNull();
    expect(credentialRowLabel('emailer', 'api_key').note).toBeNull();
    expect(credentialRowLabel('acme', 'oauth_client').note).toContain('App registration');
  });
});
