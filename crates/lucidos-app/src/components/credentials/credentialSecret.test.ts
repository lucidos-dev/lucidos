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
      '{"client_id":"id","client_secret":"sec","auth_url":"https://a","token_url":"https://t","userinfo_url":"https://u","scopes":"read write"}'
    );
    expect(f.clientId).toBe('id');
    expect(f.clientSecret).toBe('sec');
    expect(f.authUrl).toBe('https://a');
    expect(f.tokenUrl).toBe('https://t');
    expect(f.userinfoUrl).toBe('https://u');
    expect(f.scopes).toBe('read write');
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
});
