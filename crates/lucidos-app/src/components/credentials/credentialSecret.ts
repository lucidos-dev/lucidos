import type { AuthType } from '../../store/types';
import { OAUTH_WEB_CALLBACK_PATH } from '../../generated/gateway-constants';

/** Editable secret field values for every auth type, flattened into one shape.
 *  Only the subset relevant to the active auth type is populated/read. */
export interface CredentialFields {
  // Single-value types (api_key, bearer, basic, email_password)
  value: string;
  // password type
  username: string;
  password: string;
  // oauth_client type
  clientId: string;
  /** Blank means a PUBLIC client: the engine omits the secret and uses PKCE. */
  clientSecret: string;
  authUrl: string;
  tokenUrl: string;
  userinfoUrl: string;
  /** `'POST'` for a provider whose userinfo endpoint refuses GET (Dropbox's
   *  `users/get_current_account`). Blank means the OIDC default, GET. */
  userinfoMethod: string;
  /** Extra authorization-URL parameters, `key=value&key=value`. Blank means the
   *  engine's default (`access_type=offline&prompt=consent`); this is where a
   *  provider's own spelling of "issue a refresh token" goes, e.g. Dropbox's
   *  `token_access_type=offline`. */
  authorizeParams: string;
  /** Blank means the default loopback callback URI. */
  redirectUri: string;
  /** *Web redirect URIs*, one per line: `https://<origin>/~/oauth/callback`,
   *  for a browser on a computer not running Lucidos. Each needs a Web client,
   *  which has a client secret. */
  webRedirectUris: string;
}

export function emptyFields(): CredentialFields {
  return {
    value: '',
    username: '',
    password: '',
    clientId: '',
    clientSecret: '',
    authUrl: '',
    tokenUrl: '',
    userinfoUrl: '',
    userinfoMethod: '',
    authorizeParams: '',
    redirectUri: '',
    webRedirectUris: '',
  };
}

/** Parse a stored secret value into editable field values, so the edit form
 *  can pre-fill what's already saved. `password` / `oauth_client` secrets are
 *  JSON blobs; everything else is the raw value. Malformed JSON degrades to
 *  empty fields rather than throwing. */
export function parseSecret(authType: AuthType, authValue: string): CredentialFields {
  const f = emptyFields();
  if (authType === 'oauth_client') {
    const p = safeJson(authValue);
    f.clientId = pick(p, 'client_id');
    f.clientSecret = pick(p, 'client_secret');
    f.authUrl = pick(p, 'auth_url');
    f.tokenUrl = pick(p, 'token_url');
    f.userinfoUrl = pick(p, 'userinfo_url');
    f.userinfoMethod = pick(p, 'userinfo_method');
    f.authorizeParams = pick(p, 'authorize_params');
    f.redirectUri = pick(p, 'redirect_uri');
    f.webRedirectUris = pickList(p, 'web_redirect_uris').join('\n');
  } else if (authType === 'password') {
    const p = safeJson(authValue);
    f.username = pick(p, 'username');
    f.password = pick(p, 'password');
  } else {
    f.value = authValue;
  }
  return f;
}

/** Build the `auth_value` string the API persists, from edited field values.
 *  Mirrors the shapes `parseSecret` reads and the engine's credential store
 *  expects (`{username,password}` for password, `{client_id,...}` for oauth).
 *
 *  Returns `''` when every secret field is blank, which is the engine's
 *  "keep the current secret" signal on edit — so clearing a pre-filled field
 *  preserves the stored secret uniformly across all auth types (matching the
 *  single-value case) rather than overwriting it with empty values. */
export function buildSecret(authType: AuthType, f: CredentialFields): string {
  if (authType === 'oauth_client') {
    if (
      !f.clientId &&
      !f.clientSecret &&
      !f.authUrl &&
      !f.tokenUrl &&
      !f.userinfoUrl &&
      !f.userinfoMethod &&
      !f.authorizeParams &&
      !f.redirectUri &&
      !f.webRedirectUris.trim()
    ) {
      return '';
    }
    const blob: Record<string, string | string[]> = { client_id: f.clientId };
    // A blank client secret is meaningful, not missing: the engine reads its
    // absence as "public client" and authenticates the exchange with PKCE
    // instead. Clearing the field on an existing credential is how you switch a
    // confidential connection to a public one.
    if (f.clientSecret) blob.client_secret = f.clientSecret;
    if (f.authUrl) blob.auth_url = f.authUrl;
    if (f.tokenUrl) blob.token_url = f.tokenUrl;
    if (f.userinfoUrl) blob.userinfo_url = f.userinfoUrl;
    // Only when it differs from the default. Storing `GET` explicitly would put
    // a key on every credential to say what its absence already says.
    if (f.userinfoMethod && f.userinfoMethod.toUpperCase() !== 'GET') {
      blob.userinfo_method = f.userinfoMethod.toUpperCase();
    }
    if (f.authorizeParams) blob.authorize_params = f.authorizeParams;
    if (f.redirectUri) blob.redirect_uri = f.redirectUri;
    const webRedirectUris = webRedirectUriLines(f.webRedirectUris);
    if (webRedirectUris.length > 0) blob.web_redirect_uris = webRedirectUris;
    return JSON.stringify(blob);
  }
  if (authType === 'password') {
    if (!f.username && !f.password) return '';
    return JSON.stringify({ username: f.username, password: f.password });
  }
  return f.value;
}

/** The web redirect URIs a field holds: one per line, trimmed, no blanks and
 *  no repeats. */
export function webRedirectUriLines(text: string): string[] {
  return [...new Set(text.split('\n').map((line) => line.trim()).filter(Boolean))];
}

/** The web redirect URI for a page at `origin`, the one a provider returns this
 *  browser to. It must match the engine's choice character for character. */
export function webRedirectUriFor(origin: string): string {
  return `${origin}${OAUTH_WEB_CALLBACK_PATH}`;
}

/** Whether a secret field holds a *secret reference* rather than the value.
 *  The engine's `SecretField::parse` decides the same way, by the scheme. */
export function isSecretReference(value: string): boolean {
  return value.startsWith('op://');
}

function safeJson(s: string): Record<string, unknown> {
  try {
    const v = JSON.parse(s);
    return v && typeof v === 'object' ? (v as Record<string, unknown>) : {};
  } catch {
    return {};
  }
}

function pick(o: Record<string, unknown>, k: string): string {
  const v = o[k];
  return typeof v === 'string' ? v : '';
}

function pickList(o: Record<string, unknown>, k: string): string[] {
  const v = o[k];
  return Array.isArray(v) ? v.filter((item): item is string => typeof item === 'string') : [];
}
