import { describe, it, expect } from 'vitest';
import { parseSecret, buildSecret, emptyFields } from './credentialSecret';

describe('parseSecret', () => {
  it('returns the raw value for single-value types', () => {
    expect(parseSecret('api_key', 'sk-123').value).toBe('sk-123');
    expect(parseSecret('bearer', 'tok').value).toBe('tok');
    expect(parseSecret('email_password', 'hunter2').value).toBe('hunter2');
  });

  it('splits a password JSON blob into username/password', () => {
    const f = parseSecret('password', '{"username":"alice","password":"s3cret"}');
    expect(f.username).toBe('alice');
    expect(f.password).toBe('s3cret');
  });

  it('extracts oauth_client fields including optional endpoints', () => {
    const f = parseSecret(
      'oauth_client',
      '{"client_id":"id","client_secret":"sec","auth_url":"https://a","token_url":"https://t","userinfo_url":"https://u","scopes":"read write","redirect_uri":"http://localhost:14981/oauth/callback"}'
    );
    expect(f.clientId).toBe('id');
    expect(f.clientSecret).toBe('sec');
    expect(f.authUrl).toBe('https://a');
    expect(f.tokenUrl).toBe('https://t');
    expect(f.userinfoUrl).toBe('https://u');
    expect(f.scopes).toBe('read write');
    expect(f.redirectUri).toBe('http://localhost:14981/oauth/callback');
  });

  it('leaves clientSecret blank for a public-client credential', () => {
    // No client_secret stored = public client (engine uses PKCE). The blank
    // must survive the round trip, not be confused with "unset endpoint".
    const f = parseSecret('oauth_client', '{"client_id":"id","token_url":"https://t"}');
    expect(f.clientId).toBe('id');
    expect(f.clientSecret).toBe('');
    expect(f.redirectUri).toBe('');
  });

  it('degrades malformed JSON to empty fields instead of throwing', () => {
    const f = parseSecret('password', 'not json');
    expect(f.username).toBe('');
    expect(f.password).toBe('');
  });
});

describe('buildSecret', () => {
  it('returns the raw value for single-value types', () => {
    const f = { ...emptyFields(), value: 'sk-123' };
    expect(buildSecret('api_key', f)).toBe('sk-123');
    expect(buildSecret('email_password', f)).toBe('sk-123');
  });

  it('encodes password as a {username,password} blob', () => {
    const f = { ...emptyFields(), username: 'alice', password: 's3cret' };
    expect(JSON.parse(buildSecret('password', f))).toEqual({
      username: 'alice',
      password: 's3cret',
    });
  });

  it('encodes oauth_client and omits blank optional endpoints', () => {
    const f = { ...emptyFields(), clientId: 'id', clientSecret: 'sec', tokenUrl: 'https://t', authUrl: 'https://a' };
    expect(JSON.parse(buildSecret('oauth_client', f))).toEqual({
      client_id: 'id',
      client_secret: 'sec',
      auth_url: 'https://a',
      token_url: 'https://t',
    });
  });

  it('returns "" (keep current secret) when every field is blank, for all types', () => {
    const f = emptyFields();
    expect(buildSecret('api_key', f)).toBe('');
    expect(buildSecret('email_password', f)).toBe('');
    expect(buildSecret('password', f)).toBe('');
    expect(buildSecret('oauth_client', f)).toBe('');
  });

  it('still encodes a password when only one of username/password is set', () => {
    const f = { ...emptyFields(), password: 'pw' };
    expect(JSON.parse(buildSecret('password', f))).toEqual({ username: '', password: 'pw' });
  });

  it('round-trips parse -> build for oauth_client', () => {
    const stored = '{"client_id":"id","client_secret":"sec","auth_url":"https://a","token_url":"https://t"}';
    const rebuilt = buildSecret('oauth_client', parseSecret('oauth_client', stored));
    expect(JSON.parse(rebuilt)).toEqual(JSON.parse(stored));
  });

  it('encodes a redirect_uri override only when set', () => {
    const f = { ...emptyFields(), clientId: 'id', clientSecret: 'sec' };
    expect(JSON.parse(buildSecret('oauth_client', f))).not.toHaveProperty('redirect_uri');

    const overridden = { ...f, redirectUri: 'http://localhost:14981/oauth/callback' };
    expect(JSON.parse(buildSecret('oauth_client', overridden))).toEqual({
      client_id: 'id',
      client_secret: 'sec',
      redirect_uri: 'http://localhost:14981/oauth/callback',
    });
  });

  it('omits client_secret entirely for a public client', () => {
    // A blank secret is a choice, not an omission — the engine reads its
    // absence as "public client, authenticate with PKCE". Writing
    // `client_secret: ""` would work too, but omitting it keeps the stored
    // blob honest about which shape was chosen.
    const f = { ...emptyFields(), clientId: 'id', tokenUrl: 'https://t' };
    const blob = JSON.parse(buildSecret('oauth_client', f));
    expect(blob).not.toHaveProperty('client_secret');
    expect(blob).toEqual({ client_id: 'id', token_url: 'https://t' });
  });

  it('round-trips parse -> build for a public-client oauth_client', () => {
    const stored =
      '{"client_id":"id","token_url":"https://t","redirect_uri":"http://127.0.0.1:14981/oauth/callback"}';
    const rebuilt = buildSecret('oauth_client', parseSecret('oauth_client', stored));
    expect(JSON.parse(rebuilt)).toEqual(JSON.parse(stored));
  });

  // `userinfo_method` exists for Dropbox, whose userinfo endpoint is POST-only.
  // GET is the default, so it must NOT be written: a key on every credential
  // saying what its absence already says is drift waiting to happen.
  it('stores userinfo_method only when it differs from the GET default', () => {
    const base = { ...emptyFields(), clientId: 'id' };
    expect(JSON.parse(buildSecret('oauth_client', base))).not.toHaveProperty('userinfo_method');
    expect(
      JSON.parse(buildSecret('oauth_client', { ...base, userinfoMethod: 'GET' })),
    ).not.toHaveProperty('userinfo_method');
    expect(
      JSON.parse(buildSecret('oauth_client', { ...base, userinfoMethod: 'post' })).userinfo_method,
    ).toBe('POST');
  });

  it('round-trips a POST userinfo credential', () => {
    const stored = JSON.stringify({
      client_id: 'k8f2m9qxz1abc4d',
      auth_url: 'https://www.dropbox.com/oauth2/authorize',
      token_url: 'https://api.dropboxapi.com/oauth2/token',
      userinfo_url: 'https://api.dropboxapi.com/2/users/get_current_account',
      userinfo_method: 'POST',
    });
    const parsed = parseSecret('oauth_client', stored);
    expect(parsed.userinfoMethod).toBe('POST');
    expect(JSON.parse(buildSecret('oauth_client', parsed))).toEqual(JSON.parse(stored));
  });

});
