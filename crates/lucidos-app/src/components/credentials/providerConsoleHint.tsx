/**
 * The *OAuth provider registry*'s console guidance, and where it is allowed to
 * appear.
 *
 * `permissions_hint` and `console_url` used to reach exactly one surface, the
 * credential form, which is the one moment the user is registering an app.
 * They are needed again at a completely different moment: when an account came
 * back short of a scope and the fix is a checkbox in the provider's console
 * rather than another press of the same button. Pressing *Reconnect* or *Grant
 * access* without that step grants the same narrow set again, which is the hour
 * a user lost on 2026-08-07.
 *
 * Lives beside the credential form's other registry helpers because that is
 * where registry rows are rendered; the two settings surfaces import it rather
 * than growing a second copy of the link markup, so "Open the X console" reads
 * the same wherever it appears.
 */
import { type VNode } from 'preact';
import type { KnownOAuthProvider } from '../../store/types';
import { rowForService } from './oauthClientForm';

/** The provider's own app console, as a link.
 *
 *  A row with no `console_url` renders nothing: the registry cannot invent a
 *  console, and a dead "Open the console" is worse than silence. */
export function ConsoleLink({ row }: { row: KnownOAuthProvider }): VNode | null {
  if (!row.console_url) return null;
  return (
    <a class="accent-link" href={row.console_url} target="_blank" rel="noopener noreferrer">
      Open the {row.console_label ?? `${row.label} console`} ↗
    </a>
  );
}

/** What to do in the provider's console, next to the button that re-authorizes.
 *
 *  Deliberately quiet: an aside under an action, not a warning banner. The
 *  shortfall itself is already stated by whatever renders above it (the account
 *  row's missing-scope line, or the Backup page's blocked state), so this only
 *  says where to go and what to enable there. */
export function ProviderPermissionsHint({ row }: { row: KnownOAuthProvider }): VNode {
  return (
    <div class="oauth-permissions-hint">
      {row.permissions_hint && <p>{row.permissions_hint}</p>}
      <ConsoleLink row={row} />
    </div>
  );
}

/** The registry row whose guidance belongs beside a re-authorization button, or
 *  undefined for "show nothing".
 *
 *  Three ways to get nothing, and each is a state the two call sites are
 *  genuinely in:
 *
 *  - **No shortfall.** A working account gets no console lecture. This is the
 *    gate that keeps the guidance tied to a problem the user actually has.
 *  - **No row.** A *derived provider*, or an install whose system-knowhow is
 *    not staged, so the registry is empty. The button still works; there is
 *    simply nothing provider-specific to say.
 *  - **A row with neither a hint nor a console URL.** Rendering the wrapper for
 *    it would leave an empty box under the line.
 *
 *  `providers` is whatever the `knownOAuthProviders` signal has loaded, so a
 *  registry still in flight passes an empty array and the aside appears when it
 *  lands. Nothing waits on it: this is guidance beside an action, and a loader
 *  there would be louder than the guidance. */
export function reauthorizationHint(
  providers: KnownOAuthProvider[],
  provider: string,
  hasShortfall: boolean,
): KnownOAuthProvider | undefined {
  if (!hasShortfall) return undefined;
  // The same id lookup the credential form does, deliberately shared: a second
  // copy would be free to disagree about a derived provider, and then the form
  // and this aside would answer differently for one name.
  const row = rowForService(providers, provider);
  if (!row) return undefined;
  return row.permissions_hint || row.console_url ? row : undefined;
}
