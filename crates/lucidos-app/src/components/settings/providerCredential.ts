import type { CredentialInfo, Loadable } from '../../store/types';
import type { UpdateCredentialBody } from '../../api/client/settings';

export function findProviderCredential(
  credLoadable: Loadable<CredentialInfo[]>,
  service: string
): CredentialInfo | undefined {
  if (credLoadable.status !== 'loaded') return undefined;
  return credLoadable.data.find(
    (c) => c.service_name === service && c.auth_type !== 'oauth_client'
  );
}

export function providerKeyRescope(
  existing: CredentialInfo | undefined,
  url: string,
): UpdateCredentialBody | null {
  const target = url.trim();
  if (!existing || !target || existing.base_urls.includes(target)) return null;
  return {
    base_urls: [target],
    auth_type: existing.auth_type,
    auth_header: existing.auth_header,
    env_var_name: existing.env_var_name ?? undefined,
  };
}
