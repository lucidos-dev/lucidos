import type { AuthType } from '../../store/types';
import { OAUTH_WEB_CALLBACK_PATH } from '../../generated/gateway-constants';

export interface CredentialFields {
  value: string;
  username: string;
  password: string;
  clientId: string;
  clientSecret: string;
  authUrl: string;
  tokenUrl: string;
  userinfoUrl: string;
  userinfoMethod: string;
  authorizeParams: string;
  redirectUri: string;
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
    if (f.clientSecret) blob.client_secret = f.clientSecret;
    if (f.authUrl) blob.auth_url = f.authUrl;
    if (f.tokenUrl) blob.token_url = f.tokenUrl;
    if (f.userinfoUrl) blob.userinfo_url = f.userinfoUrl;
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

export function webRedirectUriLines(text: string): string[] {
  return [...new Set(text.split('\n').map((line) => line.trim()).filter(Boolean))];
}

export function webRedirectUriFor(origin: string): string {
  return `${origin}${OAUTH_WEB_CALLBACK_PATH}`;
}

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
