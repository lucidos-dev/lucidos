/**
 * The `oauth_client` half of the credential form.
 *
 * The submit guard is the one that matters: the endpoint section has always been
 * LABELLED "(required)" and never enforced it, so a client saved with both URLs
 * blank was accepted and failed on the next press of Connect with "Missing
 * auth_url in OAuth credentials", one screen away from the field that caused it.
 */
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

  it('accepts a blank client secret, which selects a public client', () => {
    // Not an omission: a blank secret is how Lucidos is expressed as a public
    // client authenticating with PKCE, the right shape for a desktop app.
    expect(oauthClientSubmitError(fields({ ...COMPLETE, clientSecret: '' }), false)).toBeNull();
  });

  it('refuses the endpoint-less client the old pair rule let through', () => {
    // The exact save that produced the reported toast. The old check was
    // "if one URL, then both", which both-blank passes.
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
    // `buildSecret` reads an all-blank form as "preserve the stored secret", so
    // enforcing here would make it impossible to edit anything else about an
    // existing credential without re-entering the endpoints.
    expect(oauthClientSubmitError(emptyFields(), true)).toBeNull();
  });

  it('still refuses a half-filled endpoint pair on an edit', () => {
    // The one guard the old pair rule got right, and the one an edit still
    // needs: a form with ANY field filled rebuilds the whole secret, so blanking
    // one URL while filling the other drops it from a credential that worked.
    expect(oauthClientSubmitError(fields({ ...COMPLETE, tokenUrl: '' }), true)).toContain(
      'Token URL is required',
    );
    expect(oauthClientSubmitError(fields({ ...COMPLETE, authUrl: '' }), true)).toContain(
      'Authorization URL is required',
    );
    // Both present is fine, and so is an edit that changes only the secret.
    expect(oauthClientSubmitError(fields(COMPLETE), true)).toBeNull();
    expect(oauthClientSubmitError(fields({ clientSecret: 'new' }), true)).toBeNull();
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
    // The point of reopening: the connection is not abandoned, it continues.
    expect(notice).toContain('continues');
  });

  it('joins several with a final and', () => {
    expect(describeMissingFields(['auth_url', 'token_url'])).toContain(
      'Authorization URL and Token URL',
    );
  });

  it('passes an unrecognized field through rather than dropping it', () => {
    // A field the engine adds later must still be reported, even unlabelled: a
    // silent omission would leave the notice claiming nothing is wrong.
    expect(describeMissingFields(['some_future_field'])).toContain('some_future_field');
  });

  it('says save, not fill in, once the registry has supplied every missing field', () => {
    // The reported bug: repairing a Dropbox registration whose stored secret
    // held only a client_id opened a form with both endpoints ALREADY prefilled
    // from the registry, under a notice telling the user to fill them in. The
    // autofill had worked and the screen said it hadn't.
    const notice = describeMissingFields(['auth_url', 'token_url'], {
      auth_url: COMPLETE.authUrl,
      token_url: COMPLETE.tokenUrl,
    });
    expect(notice).toContain('was missing Authorization URL and Token URL');
    expect(notice).toContain('filled it in below');
    expect(notice).toContain('saving continues the connection');
    // The clause that sends the user hunting for a value already on screen.
    expect(notice).not.toContain('could not start');
  });

  it('names only what the user still has to enter when the prefill is partial', () => {
    // A registry row supplies endpoints, never a Client ID: that one only exists
    // once an app is registered with the provider. Reporting the endpoints as
    // missing here would send the user hunting for URLs already on screen.
    const notice = describeMissingFields(['client_id', 'auth_url', 'token_url'], {
      auth_url: COMPLETE.authUrl,
      token_url: COMPLETE.tokenUrl,
    });
    expect(notice).toContain('is missing Client ID');
    expect(notice).not.toContain('Authorization URL');
    expect(notice).toContain('Fill it in below');
  });

  it('keeps the fill-it-in wording for a provider the registry does not know', () => {
    // Nothing was prefilled, so the user really does have to type both URLs.
    const notice = describeMissingFields(['auth_url', 'token_url'], {});
    expect(notice).toContain('is missing Authorization URL and Token URL');
    expect(notice).toContain('Fill it in below');
  });

  it('counts a whitespace-only value as still missing', () => {
    // The engine decided the field was missing by trimming it
    // (`missing_flow_fields`), and `oauthClientSubmitError` refuses to save one.
    // Calling it supplied here would promise a save the guard then refuses.
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

/**
 * A repair must load the credential it is repairing.
 *
 * Saving rebuilds the whole `auth_value` from the form, so a repair rendered
 * against a blank form would write back only what the request happened to seed.
 * A confidential client would lose its `client_secret` and start failing the
 * token exchange; a provider the registry does not know would lose its
 * endpoints, scopes and redirect override too. Both are silent at save time.
 *
 * Source-scan because the failure is in which component renders, and mounting
 * the modal would pull in the credential API, the inline-form store and the
 * secret loader to assert one routing decision.
 */
describe('the repair path renders against the stored credential', () => {
  const source = readFileSync(
    resolve(dirname(fileURLToPath(import.meta.url)), '../CredentialModal.tsx'),
    'utf8',
  ).replace(/\/\*[\s\S]*?\*\//g, '');

  it('routes a request carrying existing_credential_id through the loader', () => {
    expect(source).toMatch(/existing_credential_id/);
    // The loader fetches the stored secret; the plain create branch does not.
    const repair = source.slice(
      source.indexOf('const repairing'),
      source.indexOf('editing={undefined}'),
    );
    expect(repair).toContain('CredentialStoredLoader');
    expect(repair).toContain('credentialId={repairing}');
    expect(repair).toContain('request={form.request}');
  });

  it('keeps the repair on the create rules rather than the edit relaxation', () => {
    // `editing` unset is what makes the endpoints genuinely required, which is
    // the whole point of reopening the form.
    expect(source).toContain('editing={request ? undefined : credentialId}');
  });

  it('gives the notice the values the form is showing, not the stored ones', () => {
    // Without them the notice describes the broken ROW, which for a known
    // provider disagrees with the form the user is looking at.
    expect(source).toContain('auth_url: initialAuthUrl');
    expect(source).toContain('token_url: initialTokenUrl');
    expect(source).toContain('client_id: initialFields.clientId');
  });

  it('opens the endpoint section when the repair reopened the form for it', () => {
    // Prefilled endpoints normally collapse. A repair that named them is the
    // exception: the notice says those two fields are the reason the form is on
    // screen, so shutting them away is what made the autofill look inert.
    expect(source).toContain('open={!hasPrefilledEndpoints || repairNamedEndpoints}');
  });

  it('does not write the base URL through a ref behind a controlled input', () => {
    // The input is controlled, so a ref write is reverted by the next render
    // and the selected base provider's base_url never reaches the save.
    expect(source).not.toMatch(/baseUrlRef\.current\.value\s*=/);
    expect(source).toContain('setBaseUrl(row.base_url)');
  });
});
