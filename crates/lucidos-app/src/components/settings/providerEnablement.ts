/**
 * What a provider's enable switch shows, and what pressing it must do.
 *
 * Pure over its inputs so the matrix is testable without the Providers page.
 * Two sources feed it, and neither alone is enough. `/health` says which
 * providers the engine actually installed, which is the only honest answer to
 * "is this on". The `provider_enabled_<id>` preference is what tells a provider
 * the user switched off from one they never set up, since both are absent from
 * that list.
 */

import type { SwitchableProvider } from '../../store/actions/preferences';

export interface ProviderEnablementInput {
  /** `/health.configured_providers`, once it has answered. A `null` list is
   *  UNKNOWN, and the caller must hold the block unloaded rather than passing
   *  it here: see `providerBlockLoaded`. */
  installed: string[];
  /** The preference reads an explicit `false`. */
  switchedOff: boolean;
}

/** `on`: the engine is serving it. `switched-off`: configured once, switched
 *  off since, key still stored. `not-set-up`: nothing to switch on yet. */
export type ProviderState = 'on' | 'switched-off' | 'not-set-up';

export function providerState(
  id: SwitchableProvider,
  input: ProviderEnablementInput,
): ProviderState {
  if (input.installed.includes(id)) return 'on';
  if (input.switchedOff) return 'switched-off';
  return 'not-set-up';
}

/** Whether the block knows enough to draw a switch position at all.
 *
 *  Unknown is not "off". A provider the launch environment configured stores
 *  nothing on this page, so before `/health` answers there is no local evidence
 *  it exists. Guessing renders a running provider as never set up. Worse, a
 *  press against that guess resolves to a local expand and writes nothing, so
 *  the user's switch-off is silently dropped. A `LoadableToggle` placeholder
 *  costs one frame and cannot be pressed. */
export function providerBlockLoaded(
  installed: string[] | null,
  preferencesLoaded: boolean,
): boolean {
  return installed !== null && preferencesLoaded;
}

/** The same three states for TypeSafe (Jev), which has no `/health` row.
 *
 *  Jev holds no conversation, so it has no `ProviderKind` and never appears in
 *  `configured_providers` (ADR 0220). A stored `typesafe` credential stands in
 *  for that list: it is the one thing this page can see that says the provider
 *  is set up. So a workspace that stored no key reads `not-set-up` and draws
 *  the switch off, exactly as xAI does with no key.
 *
 *  **The check order is the reverse of `providerState`'s, and has to be.**
 *  `/health` drops a provider the moment it is switched off, so `installed`
 *  already folds the preference in and can be asked first. A credential does
 *  not: it sits there while the provider is parked. So the preference is asked
 *  first here, and the credential decides only whether there is anything to
 *  park.
 *
 *  **A launch-environment key is a blind spot.** The engine accepts
 *  `TYPESAFE_API_KEY`, which is not a credential row and so is not in the list
 *  this page reads. Such a workspace reads `not-set-up` while Jev really runs.
 *  The two per-site switches stay operable with no stored credential for
 *  exactly that reason, and remain the way to stop Jev there. */
export function typeSafeProviderState(input: {
  /** Whether a `typesafe` credential is stored. */
  keyStored: boolean;
  /** The `provider_enabled_typesafe` preference reads an explicit `false`. */
  switchedOff: boolean;
}): ProviderState {
  if (!input.keyStored) return 'not-set-up';
  return input.switchedOff ? 'switched-off' : 'on';
}

/** Whether the TypeSafe block knows enough to draw a switch position.
 *
 *  Both reads, for the reason `providerBlockLoaded` needs both: unloaded
 *  credentials read as no key, so a guess in that window draws a configured
 *  provider as never set up. */
export function typeSafeBlockLoaded(
  credentialsLoaded: boolean,
  preferencesLoaded: boolean,
): boolean {
  return credentialsLoaded && preferencesLoaded;
}

/** What a toggle press means.
 *
 *  `enable` / `disable` write the preference; `expand` / `collapse` are local
 *  disclosure and write nothing. The split is what keeps a switch off a
 *  provider that was never configured. Writing `false` there would silently
 *  veto a key added later by env var, over a switch the user only pressed to
 *  peek at the fields. */
export type SwitchAction = 'enable' | 'disable' | 'expand' | 'collapse';

export function switchAction(state: ProviderState, next: boolean): SwitchAction {
  if (next) return state === 'switched-off' ? 'enable' : 'expand';
  return state === 'on' ? 'disable' : 'collapse';
}

/** Whether the block's config rows are showing, and so where the switch sits.
 *
 *  `override` is the press the user just made, held until the engine agrees.
 *  It is `null` when there is none. Both directions need it. Switching ON a
 *  provider with no key installs nothing, so the derived state would hide the
 *  very fields the user opened. Switching one OFF takes a rebuild and a
 *  `/health` probe: the derived state holds the switch on until that lands,
 *  and the press reads as ignored. */
export function blockIsOpen(state: ProviderState, override: boolean | null): boolean {
  return override ?? state === 'on';
}

/** Whether a held press has been overtaken by reality and should be dropped.
 *
 *  Dropping it is what lets the engine correct a press that did not take: a
 *  saved key the provider rejected leaves the block open on an override that
 *  now agrees with nothing, and only the derived state knows that. */
export function overrideIsSettled(state: ProviderState, override: boolean | null): boolean {
  return override !== null && override === (state === 'on');
}
