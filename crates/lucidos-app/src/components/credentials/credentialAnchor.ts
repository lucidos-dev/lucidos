/**
 * Where a deep link into Settings > Accounts lands on one credential.
 *
 * Its own module, importing nothing. `store/actions/menu.ts` needs it, and
 * reaching into `CredentialItem.tsx` for it closed a cycle: that component
 * imports `store/actions/credentials.ts`, which imports `menu.ts` back.
 *
 * Every binding in that loop is a hoisted function today, so it survives. One
 * module-level `const` anywhere in it turns the loop into a boot-time error.
 */

/** The `data-search-anchor` value for a credential row.
 *
 *  Keyed by `id`, never by `service_name`. The scroll effect in `SettingsView`
 *  interpolates the target into a `querySelector` string, and a name is
 *  user-typed: one quote in it and the selector throws. A uuid cannot. */
export function credentialAnchor(id: string): string {
  return `accounts:credential:${id}`;
}
