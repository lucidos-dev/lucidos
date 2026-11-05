/**
 * The Connect / Reconnect decisions on Settings → Accounts.
 *
 * Every case here is one of the dead ends the page shipped with: a Reconnect
 * that could not widen, a Connect that always asked for a bare sign-in however
 * it was reached, and a quick button whose label was sent as if it were a
 * provider id.
 */
import { describe, it, expect } from 'vitest';
import {
  SIGN_IN_SCOPES,
  connectScopes,
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

/** What Microsoft echoes for a token issued to the `outlook.office.com`
 *  RESOURCE: that resource's own scopes, and nothing else. */
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
    // The reported dead end. An account narrowed to account_info.read could only
    // ever ask for account_info.read again, because the engine merges a request
    // with the existing grant: granted UNION granted is a no-op. So a scope the
    // provider's console had not permitted could never be recovered, and the
    // engine's own error message names this button as the fix.
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
    // Every account connected before the column existed, and every account read
    // through an engine older than the field. Never NARROWER than the old
    // behavior, which is what makes the fallback safe.
    expect(reconnectScopes(account({ desired_scopes: null }))).toBe('account_info.read');
    expect(reconnectScopes(account({ desired_scopes: undefined }))).toBe('account_info.read');
    expect(reconnectScopes(account({ desired_scopes: '   ' }))).toBe('account_info.read');
  });
});

describe('connectScopes', () => {
  it('is a bare sign-in when nothing said what the connection is for', () => {
    expect(connectScopes()).toBe(SIGN_IN_SCOPES);
    expect(connectScopes(null)).toBe(SIGN_IN_SCOPES);
    expect(connectScopes('')).toBe(SIGN_IN_SCOPES);
  });

  it('folds in the purpose a deep link supplied', () => {
    // One consent screen instead of two. Requesting only the sign-in scopes is
    // what left a user arriving from Backup facing *Grant access* the moment
    // they got back, for the same provider they had just authorized.
    const scopes = connectScopes('files.content.write files.metadata.read');
    expect(scopes).toContain('openid');
    expect(scopes).toContain('files.content.write');
    expect(scopes).toContain('files.metadata.read');
  });

  it('does not repeat a scope the purpose already names', () => {
    expect(connectScopes('email files.content.write')).toBe(
      'openid email profile files.content.write',
    );
  });
});

describe('providerToSend', () => {
  it('sends the id for a known provider, never the label shown in the field', () => {
    // A quick button puts "Dropbox" in the field. The credential's service name
    // and the connected account's provider are both `dropbox`, so sending the
    // label would open a second connection under a name differing only in case.
    expect(providerToSend(PROVIDERS, 'Dropbox')).toBe('dropbox');
    expect(providerToSend(PROVIDERS, 'dropbox')).toBe('dropbox');
    expect(providerToSend(PROVIDERS, '  DROPBOX  ')).toBe('dropbox');
  });

  it('lowercases an unknown name, matching what the engine stores', () => {
    expect(providerToSend(PROVIDERS, '  GHealth ')).toBe('ghealth');
  });

  it('still works with an empty registry', () => {
    // The degraded state: no staged system-knowhow means no rows, and the typed
    // path has to keep connecting.
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
    // It is the user's own name for a derived connection; title-casing it would
    // be a guess, and the engine renders such ids verbatim elsewhere too.
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
    // A legacy account must not read as broken just because the column that
    // would prove otherwise did not exist when it was connected.
    expect(missingScopes(account({ desired_scopes: null }))).toEqual([]);
    expect(missingScopes(account({ desired_scopes: undefined }))).toEqual([]);
  });

  // The engine's `missing_requested_scopes` answers the same cases, so a change
  // here needs the matching one in `core/oauth_tests.rs`.

  it('reads a refresh token as offline_access granted', () => {
    // The reported bug. Microsoft issues the token for a RESOURCE, so the echo
    // lists that resource's scopes and nothing else. The shortfall then named
    // offline_access on a grant that HAD produced a refresh token. That sent
    // the user to the Entra portal twice for nothing.
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
    // The case that actually breaks renewal. Same echo as above; only the
    // evidence differs.
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
    // An engine older than the field sends nothing, which is the window
    // between a new bundle and the engine restart. An unknown is not a
    // refusal, and the wrong guess here is the one that costs a console trip.
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
    // Every Connect asks for openid email profile. GitHub has no such scopes
    // and echoes none of them, so every GitHub account read as short of three.
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
