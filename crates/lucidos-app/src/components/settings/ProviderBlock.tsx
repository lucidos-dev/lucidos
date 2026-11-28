import { ProviderSpendCapAction, ProviderSpendCapStatus } from './ProviderSpendCap';
import { useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { configuredProviders, preferences } from '../../store/store';
import {
  providerSwitchedOff,
  setProviderEnabled,
  type SwitchableProvider,
} from '../../store/actions/preferences';
import { ProviderBlockFrame } from './ProviderBlockFrame';
import {
  blockIsOpen,
  overrideIsSettled,
  providerBlockLoaded,
  providerState,
  switchAction,
} from './providerEnablement';

const OVERRIDE_HOLD_MS = 8000;

export function ProviderBlock(props: {
  id: SwitchableProvider;
  label: string;
  anchor: string;
  explainer: ComponentChildren;
  detail?: ComponentChildren;
  actions?: ComponentChildren;
  hasStoredConfig: boolean;
  children: ComponentChildren;
}) {
  const installed = configuredProviders.value;
  const loaded = providerBlockLoaded(installed, preferences.value.status === 'loaded');

  const state = providerState(props.id, {
    installed: installed ?? [],
    switchedOff: providerSwitchedOff(props.id),
  });
  const [held, setHeld] = useState<{ open: boolean; wrote: boolean } | null>(null);
  const override = held?.open ?? null;
  const open = loaded && blockIsOpen(state, override);
  useEffect(() => {
    if (overrideIsSettled(state, override)) { setHeld(null); return; }
    if (!held?.wrote) return;
    const fuse = setTimeout(() => setHeld(null), OVERRIDE_HOLD_MS);
    return () => clearTimeout(fuse);
  }, [state, held]);

  function onToggle(next: boolean): void {
    const action = switchAction(state, next);
    if (action === 'enable') void setProviderEnabled(props.id, true);
    if (action === 'disable') void setProviderEnabled(props.id, false);
    setHeld({ open: next, wrote: action === 'enable' || action === 'disable' });
  }

  return (
    <ProviderBlockFrame
      label={props.label}
      anchor={props.anchor}
      explainer={props.explainer}
      detail={props.detail}
      actions={<>{props.actions}<ProviderSpendCapAction provider={props.id} /></>}
      status={<ProviderSpendCapStatus provider={props.id} />}
      switchedOff={state === 'switched-off'}
      hasStoredConfig={props.hasStoredConfig}
      loaded={loaded}
      open={open}
      onToggle={onToggle}
    >
      {props.children}
    </ProviderBlockFrame>
  );
}
