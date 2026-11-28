import type { ComponentChildren } from 'preact';
import { LoadableToggle } from '../shared/LoadableToggle';
import { Explainer } from '../shared/Explainer';
import { Disclosure } from '../shared/Disclosure';

export function ProviderBlockFrame(props: {
  label: string;
  anchor: string;
  explainer: ComponentChildren;
  detail?: ComponentChildren;
  actions?: ComponentChildren;
  status?: ComponentChildren;
  switchedOff: boolean;
  hasStoredConfig: boolean;
  loaded: boolean;
  open: boolean;
  onToggle: (next: boolean) => void;
  children: ComponentChildren;
}) {
  return (
    <>
      <div class="settings-row" data-search-anchor={props.anchor}>
        <span class="settings-row-label">
          {props.label}
          <Explainer title={props.label}>{props.explainer}</Explainer>
          {props.detail}
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
      {props.status}
      <Disclosure open={props.open}>{props.children}</Disclosure>
    </>
  );
}
