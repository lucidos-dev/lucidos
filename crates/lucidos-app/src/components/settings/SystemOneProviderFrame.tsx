import { useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { credentials, preferences } from '../../store/store';
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

/**
 * One System One provider on Settings → Models → Providers: TypeSafe,
 * Cloudflare Workers AI, or a custom endpoint.
 *
 * **Not a `ProviderBlock`.** That component pairs a `provider_enabled_*`
 * preference with `/health.configured_providers`, and a System One model is in
 * neither: it answers typed questions rather than holding a conversation, so
 * it has no `ProviderKind` (ADR 0220). It wears the same `ProviderBlockFrame`,
 * so the switch, Remove and the folded rows behave exactly as they do for xAI.
 * Only the decision differs: what this page can see stands in for `/health`.
 *
 * Each site picks a row in its own model control. This is the master switch
 * above every site picked on this provider.
 */
export function SystemOneProviderFrame(props: {
  provider: SystemOneProviderId;
  explainer: ComponentChildren;
  /** Status beside the label, such as "configured". */
  detail?: ComponentChildren;
  /** Header-row controls, such as Remove. */
  actions?: ComponentChildren;
  /** Whether this page stored anything the off state could promise to keep. */
  hasStoredConfig: boolean;
  /** The config rows. */
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
    configured: systemOneConfigured(props.provider, credLoadable, prefsLoadable),
    switchedOff: systemOneSwitchedOff(prefsLoadable, props.provider),
  });
  // The local disclosure that lets a first key be typed. A provider nobody has
  // configured has nothing to switch, so pressing its toggle writes nothing and
  // only reveals the fields. Consulted in that state alone, so a switch-off
  // made on another device still closes this block when its preference lands.
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
  const inUseBy = systemOneConsumers(props.provider, prefsLoadable, keyStored);

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
      {/* Three surfaces carry this feature, so the one holding the key says
          what the other two are doing. Without it, pasting a key looks like it
          did nothing.

          Living inside the fold keeps it honest. It never names a running site
          while the provider is off, though the preferences it reads outlive
          the switch. And it never asserts from an unloaded read, because
          `open` needs `loaded`. */}
      <div class="settings-row settings-row-child" data-search-anchor={`${spec.anchor}-usage`}>
        <span class="settings-row-label">
          In use by
          <span class="list-row-details list-row-details-prose">
            {inUseBy.length === 0
              ? 'Nothing yet. Every judgment still runs on its chat model.'
              : inUseBy.join(', ')}
          </span>
        </span>
      </div>
    </ProviderBlockFrame>
  );
}
