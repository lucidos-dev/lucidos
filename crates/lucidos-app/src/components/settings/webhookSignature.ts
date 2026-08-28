/**
 * The signature half of a webhook, as data the page can render and test.
 *
 * Everything here is pure. The engine's `HmacConfig` describes a sender's
 * scheme in fields, so the three real senders need no provider code. The
 * presets below are that same claim on this side: a scheme is a table row.
 *
 * Two things drive the shapes here. Which side invents the shared secret
 * differs per sender. And a credential the store no longer holds is a real
 * failure, which the row has to report.
 */

import type { CredentialInfo, Loadable } from '../../store/types';
import type { Webhook, WebhookHmac } from '../../api/client';

/** Which sender's scheme a hook follows. `custom` exposes the raw fields. */
export type WebhookScheme = 'github' | 'slack' | 'stripe' | 'custom';

/** Where the shared secret comes from.
 *
 *  `saved` names a credential the workspace already holds. The other two write
 *  one, and differ only in who invented the value. */
export type SecretSource = 'saved' | 'generate' | 'paste';

/** One sender, as the fields its scheme needs.
 *
 *  `secretSources` is the load-bearing part. A signing secret is shared, and
 *  the sender decides which side invents it:
 *
 *  - GitHub takes whatever secret you type into its own webhook form, so we can
 *    generate one. A hand-typed secret is the realistic weak link, so generate
 *    is the default.
 *  - Slack and Stripe issue their own and show it in their console. A secret we
 *    invent could never verify their deliveries, so generate is not offered.
 *
 *  Every scheme still offers `saved`, which is the override for a secret
 *  already sitting in a password manager. */
export interface SchemePreset {
  scheme: WebhookScheme;
  label: string;
  /** What to name a credential this scheme's secret is saved under. */
  credentialSuffix: string;
  /** The fields the scheme pins. Absent for `custom`, which pins nothing.
   *
   *  The header and the template are required here even though both are
   *  optional on the wire. A preset that pinned neither would describe no
   *  sender, and `schemeOf` matches a stored config on exactly those two. */
  hmac?: Omit<WebhookHmac, 'credential'> &
    Required<Pick<WebhookHmac, 'signature_header' | 'template'>>;
  secretSources: SecretSource[];
  defaultSource: SecretSource;
  /** Where the user finds the secret, for the schemes that issue their own. */
  hint: string;
}

/** Five minutes, which is what Slack and Stripe both document.
 *
 *  A `{timestamp}` template with no tolerance skips the replay check. That
 *  check is the only reason either scheme signs a timestamp. */
const REPLAY_TOLERANCE_SECS = 300;

export const SCHEME_PRESETS: SchemePreset[] = [
  {
    scheme: 'github',
    label: 'GitHub',
    credentialSuffix: 'github',
    hmac: {
      signature_header: 'X-Hub-Signature-256',
      algorithm: 'sha256',
      encoding: 'hex',
      prefix: 'sha256=',
      template: '{body}',
    },
    secretSources: ['generate', 'paste', 'saved'],
    defaultSource: 'generate',
    hint: 'Paste the generated secret into the Secret field on GitHub’s webhook form.',
  },
  {
    scheme: 'slack',
    label: 'Slack',
    credentialSuffix: 'slack',
    hmac: {
      signature_header: 'X-Slack-Signature',
      algorithm: 'sha256',
      encoding: 'hex',
      prefix: 'v0=',
      timestamp_header: 'X-Slack-Request-Timestamp',
      template: 'v0:{timestamp}:{body}',
      tolerance_secs: REPLAY_TOLERANCE_SECS,
    },
    secretSources: ['paste', 'saved'],
    defaultSource: 'paste',
    hint: 'Slack issues this one. Copy the Signing Secret from your app’s Basic Information page.',
  },
  {
    scheme: 'stripe',
    label: 'Stripe',
    credentialSuffix: 'stripe',
    hmac: {
      signature_header: 'Stripe-Signature',
      algorithm: 'sha256',
      encoding: 'hex',
      signature_key: 'v1',
      timestamp_key: 't',
      template: '{timestamp}.{body}',
      tolerance_secs: REPLAY_TOLERANCE_SECS,
    },
    secretSources: ['paste', 'saved'],
    defaultSource: 'paste',
    hint: 'Stripe issues this one. Copy the whsec_ value from the endpoint’s page in the dashboard.',
  },
  {
    scheme: 'custom',
    label: 'Custom',
    credentialSuffix: 'signing',
    secretSources: ['generate', 'paste', 'saved'],
    defaultSource: 'generate',
    hint: 'Describe the sender’s scheme in fields. {body} and {timestamp} are substituted.',
  },
];

/** Keyed, so a lookup cannot miss and no preset depends on list order.
 *
 *  A positional fallback was the alternative, and adding a fifth sender would
 *  have silently handed the custom scheme that sender's header and template. */
const BY_SCHEME: Record<WebhookScheme, SchemePreset> = Object.fromEntries(
  SCHEME_PRESETS.map((p) => [p.scheme, p]),
) as Record<WebhookScheme, SchemePreset>;

export function presetFor(scheme: WebhookScheme): SchemePreset {
  return BY_SCHEME[scheme];
}

/** A scheme's fields in one comparable string, with the wire's defaults filled.
 *
 *  The engine omits a field at its default, so a stored GitHub config may carry
 *  no `algorithm` while the preset spells out `sha256`. Comparing the raw
 *  objects would call those two different. */
function canonicalScheme(hmac: Omit<WebhookHmac, 'credential'>): string {
  return JSON.stringify([
    hmac.signature_header,
    hmac.algorithm ?? 'sha256',
    hmac.encoding ?? 'hex',
    hmac.prefix ?? '',
    hmac.signature_key ?? '',
    hmac.timestamp_header ?? '',
    hmac.timestamp_key ?? '',
    hmac.template ?? '{body}',
    hmac.tolerance_secs ?? null,
  ]);
}

/** A credential name for a hook, from the hook's own name and the scheme.
 *
 *  Only a suggestion: the field stays editable. Empty when the hook has no name
 *  yet, so the form never proposes a credential called `-github`. */
export function suggestedCredentialName(hookName: string, scheme: WebhookScheme): string {
  const stem = hookName.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  return stem ? `${stem}-${presetFor(scheme).credentialSuffix}` : '';
}

/** Which scheme a stored config matches, for reopening it in the editor.
 *
 *  **Every field has to match, not just the recognisable ones.** A preset
 *  scheme renders no fields, and saving it sends the preset back whole. So a
 *  half match would be rewritten to the preset on save, silently changing the
 *  digest, the prefix or the replay window.
 *
 *  Anything short of an exact match is `custom`, which shows every field and
 *  round-trips them. */
export function schemeOf(hmac: WebhookHmac): WebhookScheme {
  const target = canonicalScheme(hmac);
  const found = SCHEME_PRESETS.find((p) => p.hmac && canonicalScheme(p.hmac) === target);
  return found?.scheme ?? 'custom';
}

/** The digest, as a reader says it rather than as the wire spells it. */
export function algorithmLabel(hmac: Pick<WebhookHmac, 'algorithm'>): string {
  return hmac.algorithm === 'sha1' ? 'SHA-1' : 'SHA-256';
}

/** What a signed row says about its credential.
 *
 *  `found` carries the id, which is what the deep link into Settings > Accounts
 *  needs. `missing` is a real state: `DeliveryRefusal::CredentialMissing`
 *  exists for it, and the hook refuses every delivery until it is fixed.
 *
 *  `unknown` is the third, and leaving it out was a bug. A list that has not
 *  loaded holds no credentials, which looks exactly like a credential that is
 *  gone. So a cold open accused every signed hook of refusing its deliveries. */
export type CredentialLink =
  | { state: 'found'; name: string; id: string }
  | { state: 'missing'; name: string }
  | { state: 'unknown'; name: string };

/** Resolve the credential a signed hook names, against what the store holds.
 *
 *  Takes the `Loadable`, not its data, so "not loaded yet" cannot be spelled
 *  the same way as "not there".
 *
 *  `oauth_client` rows are skipped, matching the engine: `CredentialStore::get`
 *  excludes that type, so a hook naming one is refused at create and could
 *  never verify anyway. */
export function resolveCredential(
  hmac: Pick<WebhookHmac, 'credential'>,
  credentials: Loadable<CredentialInfo[]>,
): CredentialLink {
  const name = hmac.credential;
  if (credentials.status !== 'loaded') return { state: 'unknown', name };
  const found = credentials.data.find(
    (c) => c.service_name === name && c.auth_type !== 'oauth_client',
  );
  return found ? { state: 'found', name, id: found.id } : { state: 'missing', name };
}

/** The credentials a hook may verify with, for the picker.
 *
 *  `null` while the list is loading or has failed. The picker then says which
 *  of those it is, rather than claiming the workspace has none saved.
 *
 *  `oauth_client` rows are dropped, for the reason above: offering one would be
 *  offering a choice that cannot work. */
export function signableCredentials(
  credentials: Loadable<CredentialInfo[]>,
): CredentialInfo[] | null {
  if (credentials.status !== 'loaded') return null;
  return credentials.data.filter((c) => c.auth_type !== 'oauth_client');
}

/** What the row says when a hook's credential is gone.
 *
 *  Names the credential, because "a credential is missing" sends the reader
 *  looking, and the point is that they already know where to look. */
export function missingCredentialLine(name: string): string {
  return `No credential named ${name}, so every delivery is refused`;
}

/** A secret a response handed back, and the words for it.
 *
 *  Two produce one: an unsigned hook's bearer token, and a signing secret we
 *  minted. Never both, because a hook carries exactly one verifier kind.
 *
 *  **Only the token is genuinely one-time.** Its digest is all that is stored,
 *  so losing it means rotating. A signing secret is a saved credential, which
 *  Accounts can copy again. Calling it one-time would send the user rotating a
 *  secret they only had to look up. */
export interface SecretReveal {
  message: string;
  value: string;
  /** What the Copy button says, so the two secrets are told apart. */
  copyLabel: string;
}

/** What to reveal after a create or an update, if anything.
 *
 *  `null` when the response carried no secret, which is the ordinary case. An
 *  edit that left the verifier alone hands nothing back. */
export function secretReveal(
  result: Pick<Webhook, 'name'> & { token?: string; signing_secret?: string },
): SecretReveal | null {
  if (result.signing_secret) {
    return {
      message:
        `Signing secret for ${result.name}. Paste it into the sender’s own ` +
        'webhook form. You can copy it again from Accounts.',
      value: result.signing_secret,
      copyLabel: 'Copy secret',
    };
  }
  if (result.token) {
    return {
      message:
        `Token for ${result.name}, shown only now. ` +
        'A sender presents it as Authorization: Bearer.',
      value: result.token,
      copyLabel: 'Copy token',
    };
  }
  return null;
}
