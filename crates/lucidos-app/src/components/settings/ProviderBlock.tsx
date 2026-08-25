import { useEffect, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { configuredProviders, preferences } from '../../store/store';
import {
  providerSwitchedOff,
  setProviderEnabled,
  type SwitchableProvider,
} from '../../store/actions/preferences';
import { LoadableToggle } from '../shared/LoadableToggle';
import { Explainer } from '../shared/Explainer';
import {
  blockIsOpen,
  overrideIsSettled,
  providerBlockLoaded,
  providerState,
  switchAction,
} from './providerEnablement';

/** How long a press is held before the derived state takes the switch back.
 *  Past every probe that could agree with it: `refreshLlmConfigured` fires at
 *  once and again at 600ms, and the 5s connection poll is the backstop behind
 *  both. A fuse, not an animation, so it does not ride `--duration-scale`. */
const OVERRIDE_HOLD_MS = 8000;

/** One provider on Settings → Models → Providers: a header row that is always
 *  there, and config rows that are there only while the provider is on.
 *
 *  The page was one flat stack of every provider's fields at once, about twenty
 *  rows. The two the user wanted were lost among the eighteen they did not. The
 *  switch is what folds them away, and it is a real one: off removes the
 *  provider from the engine, with no restart.
 *
 *  Off never touches the stored key. That is the point of having a switch
 *  rather than only a Remove button: a user can park a provider and pick it up
 *  later. Remove keeps its own place in `actions`, on the header row, so a
 *  parked key can still be deleted without switching the provider back on.
 *
 *  The state machine lives in `providerEnablement.ts`, and every branch of it
 *  is unit-tested there. */
export function ProviderBlock(props: {
  /** Matches the engine's `ProviderKind` and the preference key's suffix. */
  id: SwitchableProvider;
  label: string;
  /** Search / deep-link anchor for the header row. */
  anchor: string;
  explainer: ComponentChildren;
  /** Provider-specific status beside the label, e.g. "configured (api_key)". */
  detail?: ComponentChildren;
  /** Header-row controls, left of the switch. Remove lives here. */
  actions?: ComponentChildren;
  /** Whether this page stored a credential for the provider. Decides only
   *  whether the off state may promise a kept key. */
  hasStoredConfig: boolean;
  /** The config rows. */
  children: ComponentChildren;
}) {
  // Both signals the state is derived from, read during render so the block
  // tracks them.
  const installed = configuredProviders.value;
  const loaded = providerBlockLoaded(installed, preferences.value.status === 'loaded');

  const state = providerState(props.id, {
    installed: installed ?? [],
    switchedOff: providerSwitchedOff(props.id),
  });
  // The press the user just made, held until the engine agrees with it. Every
  // press outruns its effect: a save has not happened yet, and a switch-off is
  // a rebuild plus a `/health` probe away. Held, the switch moves under the
  // finger; derived only, it would spring back for the length of a round trip.
  const [override, setOverride] = useState<boolean | null>(null);
  // Nothing is drawn open before the block knows its own state. The config
  // rows never appear under a switch position that is still a guess.
  const open = loaded && blockIsOpen(state, override);
  useEffect(() => {
    if (overrideIsSettled(state, override)) { setOverride(null); return; }
    if (override === null) return;
    // A press the engine never agrees with would otherwise be held for good,
    // and the switch would sit in a position nothing backs. Two ways in. The
    // engine can ANSWER and refuse the write, and `savePreference` toasts
    // rather than rejecting, so there is no promise to catch. Or it accepts a
    // write it cannot apply (the FailFast note in `engine/mod.rs`). Dropping
    // the hold shows what is installed, beside the toast that said so.
    const fuse = setTimeout(() => setOverride(null), OVERRIDE_HOLD_MS);
    return () => clearTimeout(fuse);
  }, [state, override]);

  function onToggle(next: boolean): void {
    const action = switchAction(state, next);
    if (action === 'enable') void setProviderEnabled(props.id, true);
    if (action === 'disable') void setProviderEnabled(props.id, false);
    setOverride(next);
  }

  return (
    <>
      <div class="settings-row" data-search-anchor={props.anchor}>
        <span class="settings-row-label">
          {props.label}
          <Explainer title={props.label}>{props.explainer}</Explainer>
          {props.detail}
          {/* Says what the OFF position means here. Without it a parked
              provider is indistinguishable from one never set up, and the key
              still sitting in the credential store is invisible. It only
              promises a kept key where this page stored one: Vertex has none,
              and an env-configured provider's key was never ours to keep. */}
          {state === 'switched-off' && (
            <span class="list-row-details">
              {props.hasStoredConfig ? 'switched off, key kept' : 'switched off'}
            </span>
          )}
        </span>
        <div class="settings-row-options">
          {props.actions}
          <LoadableToggle
            loaded={loaded}
            checked={open}
            ariaLabel={`Enable ${props.label}`}
            onChange={onToggle}
          />
        </div>
      </div>
      {open && props.children}
    </>
  );
}
