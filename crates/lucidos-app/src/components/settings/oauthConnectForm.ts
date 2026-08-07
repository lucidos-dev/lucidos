/**
 * The decisions behind the Connect / Reconnect controls on
 * Settings → Accounts → Connected accounts.
 *
 * Its own module (importing only types) so each one is unit-testable directly:
 * `SettingsView` pulls in devices, preferences, credentials, repositories and
 * the whole settings navigation, none of which a scope string should need.
 */
import type { KnownOAuthProvider, OAuthAccountInfo } from '../../store/types';

/** What a bare sign-in asks for. Every connection wants to know whose account it
 *  is, whatever else it is for. */
export const SIGN_IN_SCOPES = 'openid email profile';

/** The scope set a Connect press should request.
 *
 *  `purpose` is what the connection is FOR, handed over by whoever deep-linked
 *  here (Backup passes its upload scopes). Folding it in is the difference
 *  between one consent screen and two: without it, a user arriving from Backup
 *  completed the provider's consent screen, came back, and faced *Grant access*.
 *
 *  The sign-in scopes stay in either way, so the account still reports whose it
 *  is. Deduplicated, because a purpose may legitimately name one of them. */
export function connectScopes(purpose?: string | null): string {
  const wanted = [...SIGN_IN_SCOPES.split(/\s+/), ...(purpose ?? '').split(/\s+/)];
  return [...new Set(wanted.filter(Boolean))].join(' ');
}

/** The scope set *Reconnect* should request for an existing account.
 *
 *  The desired set, NOT the granted one. Passing `account.scopes` is what made
 *  Reconnect unable to recover a lost scope: the engine merges a request with
 *  the account's existing grant, so re-requesting the grant computed
 *  `granted UNION granted`, a no-op by construction. An account a provider had
 *  narrowed stayed narrow forever, and the engine's own Dropbox permission
 *  error names that exact button as the fix.
 *
 *  Falls back to the granted set when the account predates `desired_scopes`, or
 *  when the engine serving this page does (the window between a new bundle and
 *  the engine restart). Never narrower than the old behavior. */
export function reconnectScopes(account: OAuthAccountInfo): string {
  return account.desired_scopes?.trim() || account.scopes;
}

/** The scopes an account was asked for but did not get.
 *
 *  A real and previously invisible state: a provider may refuse part of a
 *  request (an app console that has not enabled a permission), and the account
 *  then looks exactly like one that was never asked. Only the Backup page could
 *  say otherwise, and only for its own provider, by re-deriving the shortfall
 *  from what an upload needs.
 *
 *  Empty whenever nothing was recorded, so an account from before the desired
 *  set existed reports no shortfall rather than a false one. */
export function missingScopes(account: OAuthAccountInfo): string[] {
  const desired = account.desired_scopes?.trim();
  if (!desired) return [];
  const granted = new Set(account.scopes.split(/\s+/).filter(Boolean));
  return desired.split(/\s+/).filter((s) => s && !granted.has(s));
}

/** What to put in the Connect field for `provider`.
 *
 *  A known provider gets its registry label, so arriving from Backup shows
 *  "Dropbox" rather than `dropbox` and lights the matching quick button. An
 *  unknown one is shown exactly as given: it is the user's own name for a
 *  *derived provider*, and title-casing it would be a guess. */
export function prefillLabel(providers: KnownOAuthProvider[], provider: string): string {
  const wanted = provider.trim().toLowerCase();
  return providers.find((p) => p.id.toLowerCase() === wanted)?.label ?? provider.trim();
}

/** The registry row a typed name resolves to, or undefined.
 *
 *  Matches the label as well as the id so a quick button's own text round-trips:
 *  the field holds "Dropbox" after a press, and that has to resolve to the same
 *  row as typing `dropbox`. */
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

/** The provider name to send for a typed value.
 *
 *  A known provider is addressed by its **id**, never by the label shown in the
 *  field: the id is the credential's service name and the connected account's
 *  provider, so sending "Dropbox" would make a second connection under a name
 *  that only differs in case from one that already exists. Everything else is
 *  lowercased, which is what the engine does to an OAuth client's name anyway.
 */
export function providerToSend(providers: KnownOAuthProvider[], typed: string): string {
  return matchProvider(providers, typed)?.id ?? typed.trim().toLowerCase();
}
