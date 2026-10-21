import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { DashboardTile as DashboardTileData } from '../../api/client/widgets';
import type { Dashboard as DashboardName } from '../../generated/thread-event-wire';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
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
import { SkeletonProvider } from '../shared/Skeleton';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import { DASHBOARD_CELL_REM, DASHBOARD_GAP_REM, DASHBOARD_MIN_COLUMNS, DASHBOARD_STEPS_PER_CELL, dashboardColumns, dashboardContentWidth } from '../../utils/dashboardGeometry';
import { inOrder, withTile } from './dashboardArrangement';
import { DashboardTile, TileSkeleton } from './DashboardTile';
import { useDashboardDrag } from './useDashboardDrag';

/** The tiles' DOM order: the order each first appeared in, appended to and
 *  never rearranged. A tile's place is its CSS `order`, because moving an
 *  iframe in the DOM reloads it (ADR 0419). */
export function stableTileOrder(previous: readonly string[], keys: readonly string[]): string[] {
  const present = new Set(keys);
  const kept = previous.filter((k) => present.has(k));
  const known = new Set(kept);
  return [...kept, ...keys.filter((k) => !known.has(k))];
}

const SKELETON_SIZES: TileSize[] = ['small', 'small', 'medium', 'medium'];

/** Add: the installed reusable widgets, the picker the thread menu's Add
 *  widget shows too, then the apps. A pick adds a tile to this dashboard. */
function AddWidget({ dashboard }: { dashboard: DashboardName }) {
  return (
    <OverflowMenu
      ariaLabel="Add a widget or an app to the dashboard"
      face={{ class: 'action-btn action-btn-secondary dashboard-add', children: '+ Add' }}
      onOpen={() => void loadReusableWidgets()}
      items={({ run }) => <WidgetPickerItems run={run} target={{ dashboard }} />}
    />
  );
}

/** One dashboard (ADR 0496): its own widget tiles, in a grid of square cells.
 *  The desktop rail hosts the `desktop` one, and the phone's dashboard sheet
 *  the `phone` one.
 *
 *  `hidden` is a dashboard kept mounted out of sight, as in a hidden rail. Its
 *  tiles hear they are hidden and unload after the off-screen hold, so
 *  showing it again soon finds them as they were.
 *
 *  Edit mode lays a shield over each frame. A drag or Alt plus an arrow moves
 *  a tile. The pill resizes it, and the minus removes it from this dashboard.
 *  Each drop or resize writes one arrangement. Done or Escape leaves. */
export function Dashboard({ dashboard, hidden = false }: {
  dashboard: DashboardName;
  hidden?: boolean;
}) {
  const view = dashboardFor(dashboard);
  const showLoading = useDelayedLoading(view);
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const [columns, setColumns] = useState(DASHBOARD_MIN_COLUMNS);
  const [editing, setEditing] = useState(false);
  const domOrder = useRef<string[]>([]);
  const all = view.status === 'loaded' ? view.data.tiles : [];
  const keys = all.map(dashboardTileKey);

  const commit = (tiles: DashboardTileData[]) => void arrangeDashboard(dashboard, tiles);
  const drag = useDashboardDrag({ order: keys, enabled: editing, onDrop: (order) => commit(inOrder(all, order)) });
  useEscapeStep(editing ? () => setEditing(false) : null);
  // A hidden dashboard cannot be edited, and its Escape step must not outlive it.
  useEffect(() => {
    if (hidden) setEditing(false);
  }, [hidden]);

  useEffect(() => {
    if (view.status === 'not-loaded') void rereadDashboard(dashboard);
  }, [view.status, dashboard]);

  // The dashboard's own width, never the window's: on desktop the rail's divider
  // sets it.
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

  if (view.status === 'failed') return <LoadableError noun={`the ${dashboard} dashboard`} error={view.error} />;

  const byKey = new Map(all.map((t) => [dashboardTileKey(t), t]));
  domOrder.current = stableTileOrder(domOrder.current, keys);
  const place = (key: string) => drag.shown.indexOf(key);
  // On the dashboard, since its head lines up with the grid too.
  const geometry = {
    '--dashboard-columns': String(columns),
    '--dashboard-cell': `${DASHBOARD_CELL_REM}rem`,
    '--dashboard-gap': `${DASHBOARD_GAP_REM}rem`,
    '--dashboard-steps-per-cell': String(DASHBOARD_STEPS_PER_CELL),
    '--dashboard-steps': String(columns * DASHBOARD_STEPS_PER_CELL),
  };
  const empty = view.status === 'loaded' && all.length === 0;
  const resize = (key: string, size: WidgetSize) => commit(withTile(all, key, { size }));

  return (
    <div ref={setRoot} class={`dashboard${editing ? ' is-editing' : ''}`} data-dashboard={dashboard} style={geometry}>
      <div class="dashboard-head">
        {/* The desktop rail is titled by its own aria-label and needs no words (ADR 0468). */}
        {dashboard === 'phone' && <span class="dashboard-title">Dashboard</span>}
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
              {SKELETON_SIZES.map((size, i) => <TileSkeleton key={i} size={size} />)}
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
          <div class="dashboard-grid">
            {domOrder.current.map((key) => {
              const tile = byKey.get(key);
              if (!tile) return null;
              const name = tile.label ?? tile.name;
              return (
                <DashboardTile
                  key={key}
                  tile={tile}
                  order={place(key)}
                  dashboardHidden={hidden}
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
