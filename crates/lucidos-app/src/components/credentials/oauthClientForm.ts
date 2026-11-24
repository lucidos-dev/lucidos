import { webRedirectUriLines, type CredentialFields } from './credentialSecret';
import { OAUTH_WEB_CALLBACK_PATH } from '../../generated/gateway-constants';
import type { KnownOAuthProvider } from '../../store/types';

export function oauthClientSubmitError(
  fields: CredentialFields,
  editing: boolean,
): string | null {
  if (secretRepeatsClientId(fields)) return REPEATED_SECRET_MESSAGE;
  const webRefusal = webRedirectUrisError(fields);
  if (webRefusal) return webRefusal;

  const missing = [
    !fields.authUrl.trim() && 'Authorization URL',
    !fields.tokenUrl.trim() && 'Token URL',
  ].filter(Boolean) as string[];

  if (editing) {
    return missing.length === 1
      ? `${missing[0]} is required: an OAuth flow cannot run without both endpoints.`
      : null;
  }

  if (!fields.clientId.trim()) return 'Client ID is required.';
  if (missing.length === 2) {
    return 'Authorization URL and Token URL are required. Pick a known provider to fill them in, or enter them by hand.';
  }
  if (missing.length === 1) {
    return `${missing[0]} is required: an OAuth flow cannot run without both endpoints.`;
  }
  return null;
}

const REPEATED_SECRET_MESSAGE =
  'The client secret is identical to the client ID, so the provider would refuse it. ' +
  "Paste the client secret from the provider's console, or clear the field to connect as a public client with PKCE.";

export function secretRepeatsClientId(fields: CredentialFields): boolean {
  const id = fields.clientId.trim();
  return id !== '' && id === fields.clientSecret.trim();
}

function webRedirectUrisError(fields: CredentialFields): string | null {
  const uris = webRedirectUriLines(fields.webRedirectUris);
  const malformed = uris.find((uri) => !isWebRedirectUri(uri));
  if (malformed) {
    return `${malformed} is not a web redirect URI. It must be https://<address>${OAUTH_WEB_CALLBACK_PATH}.`;
  }
  if (uris.length > 0 && !fields.clientSecret.trim()) {
    return 'A web redirect URI needs a Web client, which has a client secret. Add the secret, or remove the web redirect URIs.';
  }
  return null;
}

function isWebRedirectUri(uri: string): boolean {
  try {
    const url = new URL(uri);
    return (
      url.protocol === 'https:' &&
      url.pathname === OAUTH_WEB_CALLBACK_PATH &&
      !url.search &&
      !url.hash &&
      `${url.origin}${url.pathname}` === uri
    );
  } catch {
    return false;
  }
}

const MISSING_FIELD_LABELS: Record<string, string> = {
  client_id: 'Client ID',
  auth_url: 'Authorization URL',
  token_url: 'Token URL',
};

function joinHuman(named: string[]): string {
  return named.length === 1
    ? named[0]
    : `${named.slice(0, -1).join(', ')} and ${named[named.length - 1]}`;
}

export function describeMissingFields(
  missing: string[] | undefined,
  formValues: Record<string, string | undefined> = {},
): string | null {
  if (!missing?.length) return null;
  if (missing.includes('client_secret')) return REPEATED_SECRET_MESSAGE;
  const label = (m: string) => MISSING_FIELD_LABELS[m] ?? m;
  const stillEmpty = missing.filter((m) => !formValues[m]?.trim());
  if (stillEmpty.length === 0) {
    return (
      `This registration was missing ${joinHuman(missing.map(label))}. ` +
      'Lucidos filled it in below from what it knows about this provider, ' +
      'so saving continues the connection.'
    );
  }
  return (
    `This registration is missing ${joinHuman(stillEmpty.map(label))}, so connecting ` +
    'could not start. Fill it in below and the connection continues.'
  );
}

export function secretIsExpected(row: KnownOAuthProvider | undefined): boolean {
  return row?.client_type === 'confidential';
}

export function rowForService(
  providers: KnownOAuthProvider[],
  service: string,
): KnownOAuthProvider | undefined {
  const wanted = service.trim().toLowerCase();
  if (!wanted) return undefined;
  return providers.find((p) => p.id.toLowerCase() === wanted);
}
