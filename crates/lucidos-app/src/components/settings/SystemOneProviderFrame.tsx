import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { configuredProviders, credentials, preferences } from '../../store/store';
import { setSystemOneEnabled } from '../../store/actions/preferences';
import { findProviderCredential } from './providerCredential';
import { ProviderBlockFrame } from './ProviderBlockFrame';
import {
  switchAction,
  systemOneBlockLoaded,
  systemOneProviderState,
} from './providerEnablement';
import {
  systemOneConfigured,
  systemOneConsumers,
  systemOneSwitchedOff,
  SYSTEM_ONE_PROVIDERS,
  type SystemOneProviderId,
} from './judgmentBackend';

export function SystemOneProviderFrame(props: {
  provider: SystemOneProviderId;
  explainer: ComponentChildren;
  detail?: ComponentChildren;
  actions?: ComponentChildren;
  hasStoredConfig: boolean;
  children: ComponentChildren;
}) {
  const spec = SYSTEM_ONE_PROVIDERS[props.provider];
  const credLoadable = credentials.value;
  const prefsLoadable = preferences.value;

  const loaded = systemOneBlockLoaded(
    credLoadable.status === 'loaded',
    prefsLoadable.status === 'loaded',
  );
  const state = systemOneProviderState({
    configured: systemOneConfigured(
      props.provider,
      credLoadable,
      prefsLoadable,
      configuredProviders.value,
    ),
    switchedOff: systemOneSwitchedOff(prefsLoadable, props.provider),
  });
  const [expanded, setExpanded] = useState(false);
  const open = loaded && (state === 'on' || (state === 'not-set-up' && expanded));

  function onToggle(next: boolean): void {
    const action = switchAction(state, next);
    if (action === 'enable') void setSystemOneEnabled(spec.switchKey, true);
    if (action === 'disable') void setSystemOneEnabled(spec.switchKey, false);
    if (action === 'expand') setExpanded(true);
    if (action === 'collapse') setExpanded(false);
  }

  const keyStored = !!findProviderCredential(credLoadable, spec.credentialService);
  const inUseBy = systemOneConsumers(
    props.provider,
    prefsLoadable,
    keyStored,
    configuredProviders.value,
  );

  return (
    <ProviderBlockFrame
      label={spec.label}
      anchor={spec.anchor}
      explainer={props.explainer}
      detail={props.detail}
      actions={props.actions}
      switchedOff={state === 'switched-off'}
      hasStoredConfig={props.hasStoredConfig}
      loaded={loaded}
      open={open}
      onToggle={onToggle}
    >
      {props.children}
      <SystemOneUsageRow
        anchor={`${spec.anchor}-usage`}
        inUseBy={inUseBy}
        empty="Nothing yet. Every judgment still runs on its chat model."
      />
    </ProviderBlockFrame>
  );
}

export function SystemOneUsageRow(props: {
  anchor?: string;
  inUseBy: readonly string[];
  empty: string;
}) {
  return (
    <div class="settings-row settings-row-child" data-search-anchor={props.anchor}>
      <span class="settings-row-label">
        In use by
        <span class="list-row-details list-row-details-prose">
          {props.inUseBy.length === 0 ? props.empty : props.inUseBy.join(', ')}
        </span>
      </span>
    </div>
  );
}
