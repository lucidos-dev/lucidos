import type { ComponentChildren } from 'preact';
import { useEffect } from 'preact/hooks';
import type { OverflowMenuContext } from '../shared/OverflowMenu';
import { PinIcon } from '../shared/icons';
import { addToDashboard, removeFromDashboard, rereadDashboard } from '../../store/actions/dashboards';
import { currentDashboard, dashboardFor } from '../../store/dashboards';
import { dashboardTileKey } from '../../store/widgets';
import { widgetInstanceKey, type WidgetParams } from '../../utils/widgetParams';

/** One widget instance: what a dashboard tile names, and what a menu calls it. */
export interface WidgetInstance {
  appId: string;
  params?: WidgetParams;
  /** The instance's label, which a pin or a tile records. */
  label?: string;
  /** What the menu calls it: the label, else the widget's name. */
  name: string;
}

export type Run = OverflowMenuContext['run'];

export function Item({ icon, label, hint, disabled, onClick }: {
  icon: ComponentChildren;
  label: string;
  hint?: string;
  disabled?: boolean;
  onClick: (e: MouseEvent) => void;
}) {
  return (
    <button
      type="button"
      class="thread-overflow-item widget-menu-item"
      role="menuitem"
      aria-disabled={disabled ? 'true' : undefined}
      onClick={disabled ? undefined : onClick}
    >
      {icon}
      <span class="widget-menu-label">
        <span>{label}</span>
        {hint && <span class="widget-menu-hint">{hint}</span>}
      </span>
    </button>
  );
}

export function tileRef({ appId, params, label }: WidgetInstance) {
  return { appId, params, label };
}

/** Add to dashboard, offered in every thread, Home included, and on an app's
 *  row. It targets the dashboard this layout shows, and turns into Remove from
 *  dashboard once that dashboard holds the instance (ADR 0496). Until the
 *  dashboard is read, the row draws at once but stays inert, so it never
 *  offers the wrong action. `noun` names the tile in a failure toast. */
export function DashboardItem({ tile, noun, run }: { tile: WidgetInstance; noun: 'widget' | 'app'; run: Run }) {
  const dashboard = currentDashboard();
  const view = dashboardFor(dashboard);
  useEffect(() => {
    if (view.status === 'not-loaded') void rereadDashboard(dashboard);
  }, [view.status, dashboard]);
  const key = widgetInstanceKey(tile.appId, tile.params);
  const held = view.status === 'loaded' && view.data.tiles.some((t) => dashboardTileKey(t) === key);
  if (held) {
    return (
      <Item
        icon={<PinIcon filled />}
        label="Remove from dashboard"
        onClick={run(() => void removeFromDashboard(dashboard, tileRef(tile), tile.name, noun))}
      />
    );
  }
  return (
    <Item
      icon={<PinIcon />}
      label="Add to dashboard"
      hint={view.status === 'failed'
        ? `Couldn't read the ${dashboard} dashboard: ${view.error}`
        : `A tile on the ${dashboard} dashboard`}
      disabled={view.status !== 'loaded'}
      onClick={run(() => void addToDashboard(dashboard, tileRef(tile), tile.name, noun))}
    />
  );
}
