import type { CredentialInfo, Loadable } from '../../store/types';
import type { UpdateCredentialBody } from '../../api/client/settings';

/**
 * The provider credential stored under `service`, ignoring any OAuth client
 * registration of the same name.
 *
 * A provider block edits ONE thing: the API key or token the engine
 * authenticates that provider with. Since `auth_type` became the credential
 * discriminator, an `oauth_client` app registration is allowed to share a name
 * with it, so `find(c => c.service_name === service)` can hand back the wrong
 * row: the block would report "configured" off the registration, and Remove
 * would delete the OAuth client (breaking connected-account refresh) while
 * leaving the actual provider key in place.
 *
 * This mirrors `CredentialStore::get` in the engine, which excludes
 * `oauth_client` for exactly the same reason and is what resolves these same
 * names (`anthropic`, `openai`, `openrouter`, `xai`, `local`) at request time. The two
 * must agree, or the settings UI describes a row the engine never reads.
 */
export function findProviderCredential(
  credLoadable: Loadable<CredentialInfo[]>,
  service: string
): CredentialInfo | undefined {
  if (credLoadable.status !== 'loaded') return undefined;
  return credLoadable.data.find(
    (c) => c.service_name === service && c.auth_type !== 'oauth_client'
  );
}

/** The edit that moves a saved key to a new endpoint URL, or `null` when it is
 *  already scoped there. The engine sends the key only inside its scope, so a
 *  URL saved without this would leave the provider keyless. Used by the Local
 *  provider and the custom System One endpoint. A cleared field moves nothing:
 *  the Local engine then falls back to `LUCIDOS_LOCAL_BASE_URL`, which the page
 *  cannot see. */
export function providerKeyRescope(
  existing: CredentialInfo | undefined,
  url: string,
): UpdateCredentialBody | null {
  const target = url.trim();
  if (!existing || !target || existing.base_urls.includes(target)) return null;
  // An edit is a full replace, and an absent env var name resets to the default.
  return {
    base_urls: [target],
    auth_type: existing.auth_type,
    auth_header: existing.auth_header,
    env_var_name: existing.env_var_name ?? undefined,
  };
}
