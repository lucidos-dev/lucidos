import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import {
  describeMissingFields,
  oauthClientSubmitError,
  rowForService,
  secretIsExpected,
} from '../oauthClientForm';
import { reauthorizationHint } from '../providerConsoleHint';
import { emptyFields, type CredentialFields } from '../credentialSecret';
import type { KnownOAuthProvider } from '../../../store/types';

function fields(over: Partial<CredentialFields> = {}): CredentialFields {
  return { ...emptyFields(), ...over };
}

const COMPLETE = {
  clientId: 'abc',
  authUrl: 'https://acme.test/authorize',
  tokenUrl: 'https://api.acme.test/token',
};

describe('oauthClientSubmitError', () => {
  it('accepts a complete registration', () => {
    expect(oauthClientSubmitError(fields(COMPLETE), false)).toBeNull();
  });

  it('accepts a web redirect URI on a client with a secret', () => {
    const web = fields({
      ...COMPLETE,
      clientSecret: 's3cret',
      webRedirectUris: 'https://host.example/~/oauth/callback',
    });
    expect(oauthClientSubmitError(web, false)).toBeNull();
    expect(oauthClientSubmitError(web, true)).toBeNull();
  });

  it('refuses a web redirect URI without a secret, on create and on edit', () => {
    const web = fields({ ...COMPLETE, webRedirectUris: 'https://host.example/~/oauth/callback' });
    for (const editing of [false, true]) {
      expect(oauthClientSubmitError(web, editing)).toMatch(/client secret/);
    }
  });

  it('refuses a web redirect URI a provider could never return to', () => {
    for (const bad of [
      'http://host.example/~/oauth/callback',
      'https://host.example/oauth/callback',
      'https://host.example/~/oauth/callback/',
      'https://host.example/~/oauth/callback?x=1',
      'not a url',
    ]) {
      const web = fields({ ...COMPLETE, clientSecret: 's', webRedirectUris: bad });
      expect(oauthClientSubmitError(web, false), bad).toMatch(/is not a web redirect URI/);
    }
  });

  it('accepts a blank client secret, which selects a public client', () => {
    expect(oauthClientSubmitError(fields({ ...COMPLETE, clientSecret: '' }), false)).toBeNull();
  });

  it('refuses the endpoint-less client the old pair rule let through', () => {
    const refusal = oauthClientSubmitError(fields({ clientId: 'abc' }), false);
    expect(refusal).toContain('Authorization URL and Token URL are required');
  });

  it('names the single field when only one endpoint is missing', () => {
    expect(oauthClientSubmitError(fields({ ...COMPLETE, tokenUrl: '' }), false)).toContain(
      'Token URL is required',
    );
    expect(oauthClientSubmitError(fields({ ...COMPLETE, authUrl: '' }), false)).toContain(
      'Authorization URL is required',
    );
  });

  it('treats whitespace as blank', () => {
    expect(oauthClientSubmitError(fields({ ...COMPLETE, authUrl: '   ' }), false)).toContain(
      'Authorization URL is required',
    );
  });

  it('requires a client id', () => {
    expect(oauthClientSubmitError(fields({ ...COMPLETE, clientId: '' }), false)).toContain(
      'Client ID is required',
    );
  });

  it('demands nothing on an all-blank edit, where blank means keep what is stored', () => {
    expect(oauthClientSubmitError(emptyFields(), true)).toBeNull();
  });

  it('still refuses a half-filled endpoint pair on an edit', () => {
    expect(oauthClientSubmitError(fields({ ...COMPLETE, tokenUrl: '' }), true)).toContain(
      'Token URL is required',
    );
    expect(oauthClientSubmitError(fields({ ...COMPLETE, authUrl: '' }), true)).toContain(
      'Authorization URL is required',
    );
    expect(oauthClientSubmitError(fields(COMPLETE), true)).toBeNull();
    expect(oauthClientSubmitError(fields({ clientSecret: 'new' }), true)).toBeNull();
  });
});

describe('a client secret that repeats the client ID', () => {
  it('is refused on create and on edit, ignoring surrounding whitespace', () => {
    const repeated = fields({ ...COMPLETE, clientSecret: ' abc ' });
    for (const editing of [false, true]) {
      expect(oauthClientSubmitError(repeated, editing)).toMatch(/identical to the client ID/);
    }
  });

  it('does not refuse a different or blank secret', () => {
    expect(oauthClientSubmitError(fields({ ...COMPLETE, clientSecret: 'other' }), false)).toBeNull();
    expect(oauthClientSubmitError(fields(COMPLETE), false)).toBeNull();
  });

  it('is what a repair for the engine client_secret entry explains', () => {
    expect(describeMissingFields(['client_secret'])).toMatch(/identical to the client ID/);
  });
});

describe('describeMissingFields', () => {
  it('says nothing when the form is not a repair', () => {
    expect(describeMissingFields(undefined)).toBeNull();
    expect(describeMissingFields([])).toBeNull();
  });

  it('names one missing field in human terms', () => {
    const notice = describeMissingFields(['auth_url']);
    expect(notice).toContain('Authorization URL');
    expect(notice).toContain('continues');
  });

  it('joins several with a final and', () => {
    expect(describeMissingFields(['auth_url', 'token_url'])).toContain(
      'Authorization URL and Token URL',
    );
  });

  it('passes an unrecognized field through rather than dropping it', () => {
    expect(describeMissingFields(['some_future_field'])).toContain('some_future_field');
  });

  it('says save, not fill in, once the registry has supplied every missing field', () => {
    const notice = describeMissingFields(['auth_url', 'token_url'], {
      auth_url: COMPLETE.authUrl,
      token_url: COMPLETE.tokenUrl,
    });
    expect(notice).toContain('was missing Authorization URL and Token URL');
    expect(notice).toContain('filled it in below');
    expect(notice).toContain('saving continues the connection');
    expect(notice).not.toContain('could not start');
  });

  it('names only what the user still has to enter when the prefill is partial', () => {
    const notice = describeMissingFields(['client_id', 'auth_url', 'token_url'], {
      auth_url: COMPLETE.authUrl,
      token_url: COMPLETE.tokenUrl,
    });
    expect(notice).toContain('is missing Client ID');
    expect(notice).not.toContain('Authorization URL');
    expect(notice).toContain('Fill it in below');
  });

  it('keeps the fill-it-in wording for a provider the registry does not know', () => {
    const notice = describeMissingFields(['auth_url', 'token_url'], {});
    expect(notice).toContain('is missing Authorization URL and Token URL');
    expect(notice).toContain('Fill it in below');
  });

  it('counts a whitespace-only value as still missing', () => {
    const notice = describeMissingFields(['client_id'], { client_id: '   ' });
    expect(notice).toContain('is missing Client ID');
    expect(notice).toContain('Fill it in below');
  });
});

describe('secretIsExpected', () => {
  const confidential: KnownOAuthProvider = {
    id: 'acme',
    label: 'Acme',
    base_url: 'https://api.acme.test',
    auth_url: 'https://acme.test/authorize',
    token_url: 'https://api.acme.test/token',
    client_type: 'confidential',
  };

  it('is true only for a provider that issues confidential clients', () => {
    expect(secretIsExpected(confidential)).toBe(true);
    expect(secretIsExpected({ ...confidential, client_type: 'public' })).toBe(false);
    expect(secretIsExpected({ ...confidential, client_type: undefined })).toBe(false);
    expect(secretIsExpected(undefined)).toBe(false);
  });
});

describe('rowForService', () => {
  const providers: KnownOAuthProvider[] = [
    {
      id: 'dropbox',
      label: 'Dropbox',
      base_url: 'https://api.dropboxapi.test',
      auth_url: 'https://dropbox.test/authorize',
      token_url: 'https://api.dropboxapi.test/token',
    },
  ];

  it('matches a service name case-insensitively', () => {
    expect(rowForService(providers, 'Dropbox')?.id).toBe('dropbox');
  });

  it('misses a derived name, which is what makes the form ask', () => {
    expect(rowForService(providers, 'dropbox-archive')).toBeUndefined();
    expect(rowForService(providers, '')).toBeUndefined();
  });
});

describe('a request naming a row renders against that stored credential', () => {
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../CredentialModal.tsx'),
    'utf8',
  ).replace(/\/\*[\s\S]*?\*\//g, '');

  it('routes a request carrying existing_credential_id through the loader', () => {
    expect(source).toMatch(/existing_credential_id/);
    const targeted = source.slice(
      source.indexOf('const targetedRow'),
      source.indexOf('editing={undefined}'),
    );
    expect(targeted).toContain('CredentialStoredLoader');
    expect(targeted).toContain('credentialId={targetedRow}');
    expect(targeted).toContain('request={form.request}');
  });

  it('keeps both on the create rules rather than the edit relaxation', () => {
    expect(source).toContain('editing={request ? undefined : credentialId}');
  });

  it('gives the notice the values the form is showing, not the stored ones', () => {
    expect(source).toContain('auth_url: initialAuthUrl');
    expect(source).toContain('token_url: initialTokenUrl');
    expect(source).toContain('client_id: initialFields.clientId');
  });

  it('opens the endpoint section when the repair reopened the form for it', () => {
    expect(source).toContain('open={!hasPrefilledEndpoints || repairNamedEndpoints}');
  });

  it('does not write the base URLs through a ref behind a controlled input', () => {
    expect(source).not.toMatch(/baseUrlRef\.current\.value\s*=/);
    expect(source).toContain('setBaseUrls([row.base_url])');
  });
});

describe('reauthorizationHint', () => {
  const withHint: KnownOAuthProvider = {
    id: 'acme',
    label: 'Acme',
    base_url: 'https://api.acme.test',
    auth_url: 'https://acme.test/authorize',
    token_url: 'https://api.acme.test/token',
    console_label: 'Acme Developer Console',
    console_url: 'https://acme.test/apps',
    permissions_hint: 'Tick the permission and press Submit.',
  };
  const providers = [withHint];

  it('resolves the row when the account is genuinely short of a scope', () => {
    expect(reauthorizationHint(providers, 'acme', true)).toBe(withHint);
    expect(reauthorizationHint(providers, ' Acme ', true)).toBe(withHint);
  });

  it('shows nothing when there is no shortfall', () => {
    expect(reauthorizationHint(providers, 'acme', false)).toBeUndefined();
  });

  it('shows nothing for a provider the registry does not know', () => {
    expect(reauthorizationHint(providers, 'acme-archive', true)).toBeUndefined();
    expect(reauthorizationHint([], 'acme', true)).toBeUndefined();
    expect(reauthorizationHint(providers, '', true)).toBeUndefined();
  });

  it('shows nothing for a row with neither a hint nor a console', () => {
    const bare: KnownOAuthProvider = {
      ...withHint,
      console_url: undefined,
      console_label: undefined,
      permissions_hint: undefined,
    };
    expect(reauthorizationHint([bare], 'acme', true)).toBeUndefined();
  });

  it('resolves a row carrying only one of the two', () => {
    const hintOnly: KnownOAuthProvider = { ...withHint, console_url: undefined };
    const consoleOnly: KnownOAuthProvider = { ...withHint, permissions_hint: undefined };
    expect(reauthorizationHint([hintOnly], 'acme', true)).toBe(hintOnly);
    expect(reauthorizationHint([consoleOnly], 'acme', true)).toBe(consoleOnly);
  });
});

describe('the OAuth client form says what saving does', () => {
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../CredentialModal.tsx'),
    'utf8',
  );

  it('says saving grants no access', () => {
    expect(source).toContain('Saving this grants no access.');
  });

  it('offers no scopes field', () => {
    expect(source).not.toContain('Default Scopes');
    expect(source).not.toContain('scopesRef');
  });
});
