import type { RemoteGatewayView } from './remoteWorkspaces';

interface GatewaySwitcherProps {
  views: readonly RemoteGatewayView[];
  selected: string | null;
  disabled: boolean;
  onSelect: (gateway: string | null) => void;
}

export function GatewaySwitcher({ views, selected, disabled, onSelect }: GatewaySwitcherProps) {
  const pill = (gateway: string | null, label: string, view: RemoteGatewayView | null) => (
    <button
      key={gateway ?? ''}
      type="button"
      class={`pill-bar-btn${selected === gateway ? ' active' : ''}`}
      aria-pressed={selected === gateway}
      disabled={disabled}
      data-tooltip={view === null ? undefined : view.unreachable ?? `${view.gateway} answers`}
      onClick={() => onSelect(gateway)}
    >
      {view && (
        <span
          class={`ws-picker-dot ws-picker-dot-${view.unreachable === null ? 'healthy' : 'unhealthy'}`}
          aria-hidden="true"
        />
      )}
      {label}
    </button>
  );
  return (
    <div class="pill-bar ws-picker-gateways" role="group" aria-label="Gateway">
      {pill(null, 'This gateway', null)}
      {views.map((view) => pill(view.gateway, view.gateway, view))}
    </div>
  );
}
