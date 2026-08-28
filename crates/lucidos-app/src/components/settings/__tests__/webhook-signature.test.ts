import { describe, it, expect } from 'vitest';
import type { CredentialInfo, Loadable } from '../../../store/types';
import type { WebhookHmac } from '../../../api/client';
import {
  algorithmLabel,
  secretReveal,
  presetFor,
  resolveCredential,
  schemeOf,
  signableCredentials,
  suggestedCredentialName,
  SCHEME_PRESETS,
} from '../webhookSignature';
import {
  draftBlocker,
  draftFromHmac,
  draftToHmac,
  draftToSigningSecret,
  newSignatureDraft,
} from '../WebhookSignatureFields';

/**
 * The signature half of a webhook, without rendering any of it.
 *
 * Two claims carry the feature. A scheme is data, so the three real senders
 * round-trip through the presets. And a credential the store no longer holds
 * is reported rather than linked into nothing.
 */

function credential(over: Partial<CredentialInfo>): CredentialInfo {
  return {
    id: 'id-1',
    service_name: 'deploys-github',
    base_url: '',
    auth_type: 'secret',
    auth_header: 'Authorization',
    created_at: '2026-08-01T00:00:00Z',
    ...over,
  };
}

function loaded(rows: CredentialInfo[]): Loadable<CredentialInfo[]> {
  return { status: 'loaded', data: rows };
}

describe('the scheme presets', () => {
  it('expresses each real sender in fields, with no provider code', () => {
    const github = presetFor('github').hmac!;
    expect(github.signature_header).toBe('X-Hub-Signature-256');
    expect(github.prefix).toBe('sha256=');
    expect(github.template).toBe('{body}');

    const slack = presetFor('slack').hmac!;
    expect(slack.signature_header).toBe('X-Slack-Signature');
    expect(slack.prefix).toBe('v0=');
    expect(slack.template).toBe('v0:{timestamp}:{body}');
    expect(slack.timestamp_header).toBe('X-Slack-Request-Timestamp');

    const stripe = presetFor('stripe').hmac!;
    expect(stripe.signature_header).toBe('Stripe-Signature');
    expect(stripe.signature_key).toBe('v1');
    expect(stripe.timestamp_key).toBe('t');
    expect(stripe.template).toBe('{timestamp}.{body}');
  });

  /** A `{timestamp}` template with no tolerance skips the replay check, and
   *  that check is the only reason either scheme signs a timestamp. */
  it('gives every timestamped scheme a replay window', () => {
    for (const preset of SCHEME_PRESETS) {
      if (!preset.hmac?.template.includes('{timestamp}')) continue;
      expect(preset.hmac.tolerance_secs, preset.scheme).toBeGreaterThan(0);
    }
  });

  /** Which side invents the secret is the sender's decision. Slack and Stripe
   *  issue their own, so a secret we generated could never verify them. */
  it('offers generate only where the receiver chooses the secret', () => {
    expect(presetFor('github').secretSources).toContain('generate');
    expect(presetFor('custom').secretSources).toContain('generate');
    expect(presetFor('slack').secretSources).not.toContain('generate');
    expect(presetFor('stripe').secretSources).not.toContain('generate');
  });

  /** The override for a secret already sitting in a password manager. */
  it('lets every scheme name a credential that already exists', () => {
    for (const preset of SCHEME_PRESETS) {
      expect(preset.secretSources, preset.scheme).toContain('saved');
      expect(preset.secretSources, preset.scheme).toContain(preset.defaultSource);
    }
  });

  it('recognises a stored config as the scheme it came from', () => {
    for (const preset of SCHEME_PRESETS) {
      if (!preset.hmac) continue;
      expect(schemeOf({ credential: 'c', ...preset.hmac })).toBe(preset.scheme);
    }
    // A header nobody publishes is nobody's preset.
    expect(schemeOf({ credential: 'c', signature_header: 'X-Mine', template: '{body}' }))
      .toBe('custom');
  });

  /** A preset scheme renders no fields and saves the preset back whole, so
   *  claiming a config that only half matches would rewrite it on save.
   *
   *  The bug: a hook signing sha1 under GitHub's header read as `github`, and
   *  one Save flipped it to sha256. It then refused every delivery, with
   *  nothing on screen having said the digest moved. */
  it('claims a config only when every field matches', () => {
    const github = { credential: 'c', ...presetFor('github').hmac! };
    expect(schemeOf(github)).toBe('github');

    for (const differing of [
      { ...github, algorithm: 'sha1' as const },
      { ...github, encoding: 'base64' as const },
      { ...github, prefix: undefined },
      { ...github, tolerance_secs: 900 },
      { ...github, timestamp_header: 'X-When' },
    ]) {
      expect(schemeOf(differing), JSON.stringify(differing)).toBe('custom');
    }

    // A field the wire omits at its default still matches the preset, which
    // spells it out. Otherwise a CLI-made GitHub hook would read as custom.
    expect(schemeOf({
      credential: 'c',
      signature_header: 'X-Hub-Signature-256',
      prefix: 'sha256=',
      template: '{body}',
    })).toBe('github');
  });
});

describe('resolveCredential', () => {
  it('carries the id, which is what the deep link needs', () => {
    const found = resolveCredential({ credential: 'deploys-github' }, loaded([credential({})]));
    expect(found).toEqual({ state: 'found', name: 'deploys-github', id: 'id-1' });
  });

  /** The state `DeliveryRefusal::CredentialMissing` exists for. Every delivery
   *  is refused until it is fixed, so the row says so instead of linking. */
  it('reports a name the store does not hold', () => {
    expect(resolveCredential({ credential: 'gone' }, loaded([credential({})])))
      .toEqual({ state: 'missing', name: 'gone' });
    expect(resolveCredential({ credential: 'gone' }, loaded([])))
      .toEqual({ state: 'missing', name: 'gone' });
  });

  /** A list that has not loaded holds no credentials, which is not the same
   *  fact as a credential that is gone.
   *
   *  Collapsing the two accused every signed hook of refusing its deliveries,
   *  on every cold open. A failed fetch left the accusation up for good. */
  it('says nothing about a credential it has not looked for yet', () => {
    for (const pending of [
      { status: 'not-loaded' },
      { status: 'loading' },
      { status: 'failed', error: 'boom' },
    ] as Loadable<CredentialInfo[]>[]) {
      expect(resolveCredential({ credential: 'deploys-github' }, pending), pending.status)
        .toEqual({ state: 'unknown', name: 'deploys-github' });
      expect(signableCredentials(pending), pending.status).toBeNull();
    }
  });

  /** The engine's `CredentialStore::get` excludes `oauth_client`, so a hook
   *  naming one could never verify. Matching it here would link to a row the
   *  delivery path cannot read. */
  it('skips an oauth_client row of the same name', () => {
    const oauth = credential({ id: 'id-2', service_name: 'dropbox', auth_type: 'oauth_client' });
    expect(resolveCredential({ credential: 'dropbox' }, loaded([oauth])).state).toBe('missing');
    expect(signableCredentials(loaded([oauth, credential({})]))).toHaveLength(1);
  });
});

describe('algorithmLabel', () => {
  it('says the digest the way a reader does', () => {
    expect(algorithmLabel({ algorithm: 'sha256' })).toBe('SHA-256');
    expect(algorithmLabel({ algorithm: 'sha1' })).toBe('SHA-1');
    // The wire default, which the engine omits from the JSON.
    expect(algorithmLabel({})).toBe('SHA-256');
  });
});

describe('suggestedCredentialName', () => {
  it('derives one from the hook name and the scheme', () => {
    expect(suggestedCredentialName('Deploys', 'github')).toBe('deploys-github');
    expect(suggestedCredentialName('CI / builds', 'slack')).toBe('ci-builds-slack');
  });

  it('proposes nothing before the hook has a name', () => {
    expect(suggestedCredentialName('', 'github')).toBe('');
    expect(suggestedCredentialName('  ', 'github')).toBe('');
  });
});

describe('a signature draft', () => {
  it('becomes the preset the scheme pins', () => {
    const draft = { ...newSignatureDraft('deploys'), scheme: 'slack' as const };
    const hmac = draftToHmac(draft)!;
    expect(hmac.credential).toBe('deploys-github');
    expect(hmac.signature_header).toBe('X-Slack-Signature');
    expect(hmac.template).toBe('v0:{timestamp}:{body}');
  });

  it('round-trips a stored config back into the editor', () => {
    const stored: WebhookHmac = { credential: 'x', ...presetFor('stripe').hmac! };
    const draft = draftFromHmac(stored, 'stripe');
    // A hook that already has a secret gets no proposal to replace it.
    expect(draft.source).toBe('saved');
    expect(draftToHmac(draft)).toEqual(stored);
  });

  /** The invariant that makes the editor safe on a hook it did not create: a
   *  reopened config comes back out unchanged, whatever it holds. */
  it('round-trips a config no preset claims, field for field', () => {
    const stored: WebhookHmac = {
      credential: 'relay-signing',
      signature_header: 'X-Hub-Signature-256',
      algorithm: 'sha1',
      encoding: 'base64',
      prefix: 'sha1=',
      template: 'v1:{timestamp}:{body}',
      timestamp_header: 'X-Relay-Timestamp',
      tolerance_secs: 900,
    };
    const scheme = schemeOf(stored);
    expect(scheme, 'a config this unusual is nobody\'s preset').toBe('custom');
    expect(draftToHmac(draftFromHmac(stored, scheme))).toEqual(stored);
  });

  it('sends a pasted secret verbatim', () => {
    // The engine refuses surrounding whitespace by name. Trimming here would
    // silently change a value the sender chose.
    const draft = {
      ...newSignatureDraft('x'),
      source: 'paste' as const,
      secret: ' whsec_pad ',
    };
    expect(draftToSigningSecret(draft)).toEqual({ mode: 'provided', value: ' whsec_pad ' });
  });

  it('asks the engine to mint one, or names a saved credential', () => {
    const base = newSignatureDraft('x');
    expect(draftToSigningSecret({ ...base, source: 'generate' })).toEqual({ mode: 'generate' });
    expect(draftToSigningSecret({ ...base, source: 'saved' })).toBeUndefined();
  });

  it('says what is missing rather than sending a config that cannot verify', () => {
    expect(draftBlocker(newSignatureDraft('deploys'))).toBeNull();
    expect(draftBlocker({ ...newSignatureDraft(''), credential: '' })).toBeTruthy();
    expect(draftBlocker({ ...newSignatureDraft('x'), source: 'paste', secret: '' })).toBeTruthy();

    const custom = { ...newSignatureDraft('x'), scheme: 'custom' as const };
    expect(draftBlocker(custom), 'a custom scheme needs a header').toBeTruthy();
    expect(draftBlocker({
      ...custom,
      custom: { ...custom.custom, signature_header: 'X-Sig' },
    })).toBeNull();
    expect(draftBlocker({
      ...custom,
      custom: { ...custom.custom, signature_header: 'X-Sig', template: '{timestamp}' },
    }), 'a signature over no body says nothing about the payload').toBeTruthy();
  });

  it('drops an empty optional field rather than sending a blank one', () => {
    const custom = {
      ...newSignatureDraft('x'),
      scheme: 'custom' as const,
      custom: { ...newSignatureDraft('x').custom, signature_header: 'X-Sig' },
    };
    expect(draftToHmac(custom)).toEqual({
      credential: 'x-github',
      signature_header: 'X-Sig',
      algorithm: 'sha256',
      encoding: 'hex',
      template: '{body}',
    });
  });
});

describe('secretReveal', () => {
  /** A hook carries exactly one verifier kind, so these are never both set. */
  it('names which secret it is showing', () => {
    const secret = secretReveal({ name: 'deploys', signing_secret: 'abc' })!;
    expect(secret.value).toBe('abc');
    expect(secret.copyLabel).toBe('Copy secret');
    expect(secret.message).toContain('sender');

    const token = secretReveal({ name: 'deploys', token: 'def' })!;
    expect(token.value).toBe('def');
    expect(token.copyLabel).toBe('Copy token');
    expect(token.message).toContain('Bearer');
  });

  /** Only the token is one-time: its digest is all that is stored. A signing
   *  secret is a saved credential, and Accounts can copy it again. Calling it
   *  one-time sends the user rotating a secret they could have looked up. */
  it('claims one-time only for the secret that really is', () => {
    expect(secretReveal({ name: 'd', token: 'def' })!.message).toContain('shown only now');
    const secret = secretReveal({ name: 'd', signing_secret: 'abc' })!;
    expect(secret.message).not.toContain('shown only now');
    expect(secret.message).toContain('Accounts');
  });

  /** The ordinary edit. Renaming a hook hands nothing back to reveal. */
  it('reveals nothing when the response carried no secret', () => {
    expect(secretReveal({ name: 'deploys' })).toBeNull();
  });
});
