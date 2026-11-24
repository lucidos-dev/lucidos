import { describe, it, expect } from 'vitest';
import {
  matchProvider,
  missingScopes,
  prefillLabel,
  providerToSend,
  reconnectScopes,
} from '../oauthConnectForm';
import type { KnownOAuthProvider, OAuthAccountInfo } from '../../../store/types';

const PROVIDERS: KnownOAuthProvider[] = [
  {
    id: 'dropbox',
    label: 'Dropbox',
    base_url: 'https://api.dropboxapi.test',
    auth_url: 'https://dropbox.test/authorize',
    token_url: 'https://api.dropboxapi.test/token',
  },
  {
    id: 'google',
    label: 'Google',
    base_url: 'https://api.google.test',
    auth_url: 'https://google.test/authorize',
    token_url: 'https://api.google.test/token',
  },
];

const OUTLOOK_GRANTED = [
  'https://outlook.office.com/SMTP.Send',
  'https://outlook.office.com/IMAP.AccessAsUser.All',
  'https://outlook.office.com/Mail.Read',
].join(' ');

function account(over: Partial<OAuthAccountInfo> = {}): OAuthAccountInfo {
  return {
    id: 'a1',
    provider: 'dropbox',
    email: 'user@example.com',
    display_name: null,
    scopes: 'account_info.read',
    created_at: '2026-08-01T00:00:00Z',
    updated_at: '2026-08-01T00:00:00Z',
    ...over,
  };
}

describe('reconnectScopes', () => {
  it('requests the desired set, not the granted one', () => {
    const narrowed = account({
      scopes: 'account_info.read',
      desired_scopes: 'files.content.write files.content.read account_info.read',
    });
    expect(reconnectScopes(narrowed)).toBe(
      'files.content.write files.content.read account_info.read',
    );
    expect(reconnectScopes(narrowed)).not.toBe(narrowed.scopes);
  });

  it('falls back to the granted set when nothing was recorded', () => {
    expect(reconnectScopes(account({ desired_scopes: null }))).toBe('account_info.read');
    expect(reconnectScopes(account({ desired_scopes: undefined }))).toBe('account_info.read');
    expect(reconnectScopes(account({ desired_scopes: '   ' }))).toBe('account_info.read');
  });
});

describe('providerToSend', () => {
  it('sends the id for a known provider, never the label shown in the field', () => {
    expect(providerToSend(PROVIDERS, 'Dropbox')).toBe('dropbox');
    expect(providerToSend(PROVIDERS, 'dropbox')).toBe('dropbox');
    expect(providerToSend(PROVIDERS, '  DROPBOX  ')).toBe('dropbox');
  });

  it('lowercases an unknown name, matching what the engine stores', () => {
    expect(providerToSend(PROVIDERS, '  GHealth ')).toBe('ghealth');
  });

  it('still works with an empty registry', () => {
    expect(providerToSend([], 'Dropbox')).toBe('dropbox');
  });
});

describe('matchProvider', () => {
  it('resolves a label as well as an id, so a button press round-trips', () => {
    expect(matchProvider(PROVIDERS, 'Dropbox')?.id).toBe('dropbox');
    expect(matchProvider(PROVIDERS, 'dropbox')?.id).toBe('dropbox');
  });

  it('misses a derived name rather than guessing its base provider', () => {
    expect(matchProvider(PROVIDERS, 'ghealth')).toBeUndefined();
    expect(matchProvider(PROVIDERS, '')).toBeUndefined();
  });
});

describe('prefillLabel', () => {
  it('shows a known provider by its label', () => {
    expect(prefillLabel(PROVIDERS, 'dropbox')).toBe('Dropbox');
  });

  it('shows an unknown name exactly as given', () => {
    expect(prefillLabel(PROVIDERS, 'ghealth')).toBe('ghealth');
    expect(prefillLabel([], 'dropbox')).toBe('dropbox');
  });
});

describe('missingScopes', () => {
  it('names what the provider refused', () => {
    expect(
      missingScopes(
        account({
          scopes: 'account_info.read',
          desired_scopes: 'files.content.write account_info.read',
        }),
      ),
    ).toEqual(['files.content.write']);
  });

  it('is empty when everything asked for was granted', () => {
    expect(
      missingScopes(account({ scopes: 'a b', desired_scopes: 'b a' })),
    ).toEqual([]);
  });

  it('reports no shortfall when nothing was recorded', () => {
    expect(missingScopes(account({ desired_scopes: null }))).toEqual([]);
    expect(missingScopes(account({ desired_scopes: undefined }))).toEqual([]);
  });


  it('reads a refresh token as offline_access granted', () => {
    expect(
      missingScopes(
        account({
          provider: 'microsoft',
          scopes: OUTLOOK_GRANTED,
          desired_scopes: `${OUTLOOK_GRANTED} offline_access`,
          has_refresh_token: true,
        }),
      ),
    ).toEqual([]);
  });

  it('reports offline_access when no refresh token arrived', () => {
    expect(
      missingScopes(
        account({
          provider: 'microsoft',
          scopes: OUTLOOK_GRANTED,
          desired_scopes: `${OUTLOOK_GRANTED} offline_access`,
          has_refresh_token: false,
        }),
      ),
    ).toEqual(['offline_access']);
  });

  it('will not take an echoed offline_access for the token itself', () => {
    expect(
      missingScopes(
        account({
          scopes: 'read offline_access',
          desired_scopes: 'read offline_access',
          has_refresh_token: false,
        }),
      ),
    ).toEqual(['offline_access']);
  });

  it('stays quiet on offline_access when the engine cannot say', () => {
    expect(
      missingScopes(
        account({
          scopes: OUTLOOK_GRANTED,
          desired_scopes: `${OUTLOOK_GRANTED} offline_access`,
        }),
      ),
    ).toEqual([]);
  });

  it('never reports the sign-in scopes from the echo', () => {
    expect(
      missingScopes(
        account({
          provider: 'github',
          scopes: 'repo',
          desired_scopes: 'openid email profile repo',
        }),
      ),
    ).toEqual([]);
  });

  it('still names a refused resource scope beside a quiet meta scope', () => {
    expect(
      missingScopes(
        account({
          provider: 'microsoft',
          scopes: 'https://outlook.office.com/Mail.Read',
          desired_scopes: `${OUTLOOK_GRANTED} offline_access openid`,
          has_refresh_token: true,
        }),
      ),
    ).toEqual([
      'https://outlook.office.com/SMTP.Send',
      'https://outlook.office.com/IMAP.AccessAsUser.All',
    ]);
  });
});
