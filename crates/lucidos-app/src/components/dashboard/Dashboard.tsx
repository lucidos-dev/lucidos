import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { DashboardTile as DashboardTileData } from '../../api/client/widgets';
import type { Dashboard as DashboardName } from '../../generated/thread-event-wire';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { arrangeDashboard, removeFromDashboard, rereadDashboard } from '../../store/actions/dashboards';
import { loadReusableWidgets } from '../../store/actions/widgets';
import { dashboardFor } from '../../store/dashboards';
import type { TileSize, WidgetSize } from '../../store/types';
import { dashboardTileKey } from '../../store/widgets';
import { getRemPx } from '../../utils/dom';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { OverflowMenu } from '../shared/OverflowMenu';
import { WidgetPickerItems } from '../widgets/WidgetPickerItems';
import { useWidgetCell } from '../widgets/useWidgetCell';
import { SkeletonProvider } from '../shared/Skeleton';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import { DASHBOARD_MIN_COLUMNS, DASHBOARD_STEPS_PER_CELL, dashboardColumns, dashboardContentWidth } from '../../utils/dashboardGeometry';
import { inOrder, withTile } from './dashboardArrangement';
import { DashboardTile, TileSkeleton } from './DashboardTile';
import { useDashboardDrag } from './useDashboardDrag';
import { useDashboardSettled } from './useDashboardSettled';

export function stableTileOrder(previous: readonly string[], keys: readonly string[]): string[] {
  const present = new Set(keys);
  const kept = previous.filter((k) => present.has(k));
  const known = new Set(kept);
  return [...kept, ...keys.filter((k) => !known.has(k))];
}

const SKELETON_SIZES: TileSize[] = ['small', 'small', 'medium', 'medium'];

function AddWidget({ dashboard }: { dashboard: DashboardName }) {
  return (
    <OverflowMenu
      ariaLabel="Add a widget or an app to the dashboard"
      face={{ class: 'action-btn action-btn-secondary dashboard-add', children: '+ Add' }}
      onOpen={() => void loadReusableWidgets()}
      items={({ run }) => <WidgetPickerItems run={run} dashboard={dashboard} />}
    />
  );
}

export function Dashboard({ dashboard, hidden = false }: {
  dashboard: DashboardName;
  hidden?: boolean;
}) {
  const view = dashboardFor(dashboard);
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const [columns, setColumns] = useState(DASHBOARD_MIN_COLUMNS);
  const [editing, setEditing] = useState(false);
  const domOrder = useRef<string[]>([]);
  const all = view.status === 'loaded' ? view.data.tiles : [];
  const keys = all.map(dashboardTileKey);
  const { settled, report } = useDashboardSettled(
    view.status === 'loaded',
    hidden,
    all.filter((t) => t.kind === 'widget').map(dashboardTileKey),
  );
  const showLoading = useDelayedFlag(!hidden && view.status !== 'failed' && !settled);

  const commit = (tiles: DashboardTileData[]) => void arrangeDashboard(dashboard, tiles);
  const drag = useDashboardDrag({ order: keys, enabled: editing, onDrop: (order) => commit(inOrder(all, order)) });
  useEscapeStep(editing ? () => setEditing(false) : null);
  useEffect(() => {
    if (hidden) setEditing(false);
  }, [hidden]);

  useEffect(() => {
    if (view.status === 'not-loaded') void rereadDashboard(dashboard);
  }, [view.status, dashboard]);

  useLayoutEffect(() => {
    if (!root) return undefined;
    const measure = () => {
      const rem = getRemPx();
      setColumns(dashboardColumns(dashboardContentWidth(root.clientWidth, rem), rem));
    };
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => observer.disconnect();
  }, [root]);
  useWidgetCell(root);

  if (view.status === 'failed') return <LoadableError noun={`the ${dashboard} dashboard`} error={view.error} />;

  const byKey = new Map(all.map((t) => [dashboardTileKey(t), t]));
  domOrder.current = stableTileOrder(domOrder.current, keys);
  const place = (key: string) => drag.shown.indexOf(key);
  const skeletonSizes = view.status === 'loaded'
    ? drag.shown.flatMap((key) => byKey.get(key)?.size ?? [])
    : SKELETON_SIZES;
  const steps = columns * DASHBOARD_STEPS_PER_CELL;
  const geometry = {
    '--dashboard-columns': String(columns),
    '--dashboard-steps-per-cell': String(DASHBOARD_STEPS_PER_CELL),
    '--dashboard-steps': String(steps),
  };
  const empty = view.status === 'loaded' && all.length === 0;
  const resize = (key: string, size: WidgetSize) => commit(withTile(all, key, { size }));

  return (
    <div ref={setRoot} class={`dashboard${editing ? ' is-editing' : ''}`} data-dashboard={dashboard} style={geometry}>
      <div class="dashboard-head">
        <span class="dashboard-title">Dashboard</span>
        {!empty && <AddWidget dashboard={dashboard} />}
        {!empty && view.status === 'loaded' && (editing ? (
          <button type="button" class="action-btn dashboard-done" onClick={() => setEditing(false)}>Done</button>
        ) : (
          <button type="button" class="action-btn action-btn-secondary dashboard-edit" onClick={() => setEditing(true)}>Edit</button>
        ))}
      </div>
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={(
          <SkeletonProvider>
            <div class="dashboard-grid">
              {skeletonSizes.map((size, i) => <TileSkeleton key={i} size={size} steps={steps} />)}
            </div>
          </SkeletonProvider>
        )}
      >
        {empty ? (
          <div class="empty-state dashboard-empty">
            <p>Nothing is on this dashboard yet. Add a widget or an app. A widget shows here live, and an app opens from its icon.</p>
            <AddWidget dashboard={dashboard} />
          </div>
        ) : (
          <div class={`dashboard-grid${settled ? '' : ' is-settling'}`} inert={!settled}>
            {domOrder.current.map((key) => {
              const tile = byKey.get(key);
              if (!tile) return null;
              const name = tile.label ?? tile.name;
              return (
                <DashboardTile
                  key={key}
                  tile={tile}
                  order={place(key)}
                  steps={steps}
                  dashboardHidden={hidden}
                  onFrameState={report}
                  actions={{ dashboard, onEdit: editing ? undefined : () => setEditing(true) }}
                  editing={editing ? {
                    register: drag.register,
                    shield: drag.shield(key),
                    dragging: drag.dragging === key,
                    onMove: (delta) => drag.moveBy(key, delta),
                    onResize: (size) => resize(key, size),
                    onRemove: () => void removeFromDashboard(dashboard, { appId: tile.app_id, params: tile.kind === 'widget' ? tile.params : undefined }, name, tile.kind),
                  } : null}
                />
              );
            })}
          </div>
        )}
      </LoadingFade>
    </div>
  );
}
