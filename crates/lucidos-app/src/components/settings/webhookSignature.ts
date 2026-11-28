
import type { CredentialInfo, Loadable } from '../../store/types';
import type { Webhook, WebhookHmac } from '../../api/client';

export type WebhookScheme = 'github' | 'slack' | 'stripe' | 'custom';

export type SecretSource = 'saved' | 'generate' | 'paste';

export interface SchemePreset {
  scheme: WebhookScheme;
  label: string;
  credentialSuffix: string;
  hmac?: Omit<WebhookHmac, 'credential'> &
    Required<Pick<WebhookHmac, 'signature_header' | 'template'>>;
  secretSources: SecretSource[];
  defaultSource: SecretSource;
  hint: string;
}

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

const BY_SCHEME: Record<WebhookScheme, SchemePreset> = Object.fromEntries(
  SCHEME_PRESETS.map((p) => [p.scheme, p]),
) as Record<WebhookScheme, SchemePreset>;

export function presetFor(scheme: WebhookScheme): SchemePreset {
  return BY_SCHEME[scheme];
}

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

export function suggestedCredentialName(hookName: string, scheme: WebhookScheme): string {
  const stem = hookName.trim().toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '');
  return stem ? `${stem}-${presetFor(scheme).credentialSuffix}` : '';
}

export function schemeOf(hmac: WebhookHmac): WebhookScheme {
  const target = canonicalScheme(hmac);
  const found = SCHEME_PRESETS.find((p) => p.hmac && canonicalScheme(p.hmac) === target);
  return found?.scheme ?? 'custom';
}

export function algorithmLabel(hmac: Pick<WebhookHmac, 'algorithm'>): string {
  return hmac.algorithm === 'sha1' ? 'SHA-1' : 'SHA-256';
}

export type CredentialLink =
  | { state: 'found'; name: string; id: string }
  | { state: 'missing'; name: string }
  | { state: 'unknown'; name: string };

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

export function signableCredentials(
  credentials: Loadable<CredentialInfo[]>,
): CredentialInfo[] | null {
  if (credentials.status !== 'loaded') return null;
  return credentials.data.filter((c) => c.auth_type !== 'oauth_client');
}

export function missingCredentialLine(name: string): string {
  return `No credential named ${name}, so every delivery is refused`;
}

export interface SecretReveal {
  message: string;
  value: string;
  copyLabel: string;
}

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
