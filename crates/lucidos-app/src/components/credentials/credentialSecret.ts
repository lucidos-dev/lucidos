import type { AuthType } from '../../store/types';

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
  scopes: string;
  /** Blank means the default loopback callback URI. */
  redirectUri: string;
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
    scopes: '',
    redirectUri: '',
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
    f.scopes = pick(p, 'scopes');
    f.redirectUri = pick(p, 'redirect_uri');
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
      !f.scopes &&
      !f.redirectUri
    ) {
      return '';
    }
    const blob: Record<string, string> = { client_id: f.clientId };
    // A blank client secret is meaningful, not missing: the engine reads its
    // absence as "public client" and authenticates the exchange with PKCE
    // instead. Clearing the field on an existing credential is how you switch a
    // confidential connection to a public one.
    if (f.clientSecret) blob.client_secret = f.clientSecret;
    if (f.authUrl) blob.auth_url = f.authUrl;
    if (f.tokenUrl) blob.token_url = f.tokenUrl;
    if (f.userinfoUrl) blob.userinfo_url = f.userinfoUrl;
    if (f.scopes) blob.scopes = f.scopes;
    if (f.redirectUri) blob.redirect_uri = f.redirectUri;
    return JSON.stringify(blob);
  }
  if (authType === 'password') {
    if (!f.username && !f.password) return '';
    return JSON.stringify({ username: f.username, password: f.password });
  }
  return f.value;
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
