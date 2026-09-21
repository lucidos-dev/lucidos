import type { ComponentChildren } from 'preact';
import { LoadableToggle } from '../shared/LoadableToggle';
import { Explainer } from '../shared/Explainer';

/** The markup every provider on Settings → Models → Providers wears: a header
 *  row that is always there, and config rows that are there only while the
 *  provider is on.
 *
 *  Presentation only. It takes the switch position and gives back the press,
 *  because the two blocks above it decide differently. `ProviderBlock` derives
 *  its position from `/health`, which lags a press and needs a held override.
 *  `TypeSafeJudgmentSettings` derives its own from a credential and a
 *  preference, both of which move with the press. One frame keeps the two rows
 *  looking and behaving the same without pretending they know the same things.
 */
export function ProviderBlockFrame(props: {
  label: string;
  /** Search / deep-link anchor for the header row. */
  anchor: string;
  explainer: ComponentChildren;
  /** Provider-specific status beside the label, e.g. "configured (api_key)". */
  detail?: ComponentChildren;
  /** Header-row controls, left of the switch. Remove lives here, so a parked
   *  key can still be deleted without switching the provider back on. */
  actions?: ComponentChildren;
  /** Whether the header says the provider is parked rather than never set up.
   *  The note is the only thing that makes a stored key visible while off. */
  switchedOff: boolean;
  /** Whether this page stored a credential. Decides only whether the off state
   *  may promise a kept key: Vertex has none, and an env-configured provider's
   *  key was never ours. */
  hasStoredConfig: boolean;
  /** False until the block knows its own state. Nothing is drawn open, and the
   *  switch renders a placeholder rather than a position that is still a
   *  guess. */
  loaded: boolean;
  /** Whether the config rows are showing, and so where the switch sits. */
  open: boolean;
  onToggle: (next: boolean) => void;
  /** The config rows. */
  children: ComponentChildren;
}) {
  return (
    <>
      <div class="settings-row" data-search-anchor={props.anchor}>
        <span class="settings-row-label">
          {props.label}
          <Explainer title={props.label}>{props.explainer}</Explainer>
          {props.detail}
          {/* Says what the OFF position means here. Without it a parked
              provider is indistinguishable from one never set up, and the key
              still sitting in the credential store is invisible. */}
          {props.switchedOff && (
            <span class="list-row-details">
              {props.hasStoredConfig ? 'switched off, key kept' : 'switched off'}
            </span>
          )}
        </span>
        <div class="settings-row-options">
          {props.actions}
          <LoadableToggle
            loaded={props.loaded}
            checked={props.open}
            ariaLabel={`Enable ${props.label}`}
            onChange={props.onToggle}
          />
        </div>
      </div>
      {props.open && props.children}
    </>
  );
}
