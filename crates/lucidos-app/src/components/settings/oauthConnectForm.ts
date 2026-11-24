import type { KnownOAuthProvider, OAuthAccountInfo } from '../../store/types';

export function reconnectScopes(account: OAuthAccountInfo): string {
  return account.desired_scopes?.trim() || account.scopes;
}

const META_SCOPES = new Set(['offline_access', 'openid', 'profile', 'email']);

export function missingScopes(account: OAuthAccountInfo): string[] {
  const desired = account.desired_scopes?.trim();
  if (!desired) return [];
  const granted = new Set(account.scopes.split(/\s+/).filter(Boolean));
  return desired.split(/\s+/).filter((s) => {
    if (!s) return false;
    if (s === 'offline_access') return account.has_refresh_token === false;
    if (META_SCOPES.has(s)) return false;
    return !granted.has(s);
  });
}

export function prefillLabel(providers: KnownOAuthProvider[], provider: string): string {
  const wanted = provider.trim().toLowerCase();
  return providers.find((p) => p.id.toLowerCase() === wanted)?.label ?? provider.trim();
}

export function matchProvider(
  providers: KnownOAuthProvider[],
  typed: string,
): KnownOAuthProvider | undefined {
  const wanted = typed.trim().toLowerCase();
  if (!wanted) return undefined;
  return providers.find(
    (p) => p.id.toLowerCase() === wanted || p.label.toLowerCase() === wanted,
  );
}

export function providerToSend(providers: KnownOAuthProvider[], typed: string): string {
  return matchProvider(providers, typed)?.id ?? typed.trim().toLowerCase();
}
