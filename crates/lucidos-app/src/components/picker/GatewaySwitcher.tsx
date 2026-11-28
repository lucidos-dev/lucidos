export interface GatewayPill {
  gateway: string;
  unreachable?: string | null;
}

interface GatewaySwitcherProps {
  views: readonly GatewayPill[];
  selected: string | null;
  disabled: boolean;
  onSelect: (gateway: string | null) => void;
}

export function GatewaySwitcher({ views, selected, disabled, onSelect }: GatewaySwitcherProps) {
  const pill = (gateway: string | null, label: string, view: GatewayPill | null) => (
    <button
      key={gateway ?? ''}
      type="button"
      class={`pill-bar-btn${selected === gateway ? ' active' : ''}`}
      aria-pressed={selected === gateway}
      disabled={disabled}
      data-tooltip={view?.unreachable === undefined ? undefined : view.unreachable ?? `${view.gateway} answers`}
      onClick={() => onSelect(gateway)}
    >
      {view?.unreachable !== undefined && (
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
