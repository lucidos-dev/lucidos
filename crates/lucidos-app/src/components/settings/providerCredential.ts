import type { CredentialInfo, Loadable } from '../../store/types';

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
