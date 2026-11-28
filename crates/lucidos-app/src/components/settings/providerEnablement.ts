
import type { SwitchableProvider } from '../../store/actions/preferences';

export interface ProviderEnablementInput {
  installed: string[];
  switchedOff: boolean;
}

export type ProviderState = 'on' | 'switched-off' | 'not-set-up';

export function providerState(
  id: SwitchableProvider,
  input: ProviderEnablementInput,
): ProviderState {
  if (input.installed.includes(id)) return 'on';
  if (input.switchedOff) return 'switched-off';
  return 'not-set-up';
}

export function providerBlockLoaded(
  installed: string[] | null,
  preferencesLoaded: boolean,
): boolean {
  return installed !== null && preferencesLoaded;
}

export function systemOneProviderState(input: {
  configured: boolean;
  switchedOff: boolean;
}): ProviderState {
  if (!input.configured) return 'not-set-up';
  return input.switchedOff ? 'switched-off' : 'on';
}

export function systemOneBlockLoaded(
  credentialsLoaded: boolean,
  preferencesLoaded: boolean,
): boolean {
  return credentialsLoaded && preferencesLoaded;
}

export type SwitchAction = 'enable' | 'disable' | 'expand' | 'collapse';

export function switchAction(state: ProviderState, next: boolean): SwitchAction {
  if (next) return state === 'switched-off' ? 'enable' : 'expand';
  return state === 'on' ? 'disable' : 'collapse';
}

export function blockIsOpen(state: ProviderState, override: boolean | null): boolean {
  return override ?? state === 'on';
}

export function overrideIsSettled(state: ProviderState, override: boolean | null): boolean {
  return override !== null && override === (state === 'on');
}
