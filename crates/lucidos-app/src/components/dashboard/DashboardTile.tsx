import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { WIDGET_SIZES } from '@lucidos/engine-constants';
import type { AppDashboardTile, DashboardTile as Tile, WidgetDashboardTile } from '../../api/client/widgets';
import type { Dashboard } from '../../generated/thread-event-wire';
import { openAppById } from '../../store/actions/apps';
import { removeFromDashboard } from '../../store/actions/dashboards';
import type { TileSize, WidgetSize } from '../../store/types';
import { dashboardTileKey } from '../../store/widgets';
import { DASHBOARD_STEPS_PER_CELL } from '../../utils/dashboardGeometry';
import { AppIcon } from '../shared/AppIcon';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';
import { SkBlock } from '../shared/Skeleton';
import { useObjectMenu } from '../shared/useObjectMenu';
import { WidgetFrame } from '../widgets/WidgetFrame';
import type { FrameState } from '../widgets/widgetFrameState';
import { dashboardTileMenuItems, type DashboardTileTarget } from '../widgets/WidgetMenu';

/** How many grid steps each tile size spans, a step being a quarter cell: a
 *  widget's cells times the steps per cell, and one step for an app shortcut
 *  (ADR 0432). */
const STEPS = DASHBOARD_STEPS_PER_CELL;
export const SIZE_SPANS: Record<TileSize, { columns: number; rows: number }> = {
  quarter: { columns: 1, rows: 1 },
  small: { columns: STEPS, rows: STEPS },
  medium: { columns: 2 * STEPS, rows: STEPS },
  large: { columns: 2 * STEPS, rows: 2 * STEPS },
};

/** The size pill's letter for each size. */
const SIZE_LETTERS: Record<WidgetSize, string> = { small: 'S', medium: 'M', large: 'L' };

function gridPlace(size: TileSize, order?: number) {
  const span = SIZE_SPANS[size];
  return { order, gridColumn: `span ${span.columns}`, gridRow: `span ${span.rows}` };
}

/** What edit mode adds to a tile, and what it may do. */
export interface TileEditing {
  register: (key: string, el: HTMLElement | null) => void;
  shield: {
    onPointerDown(e: PointerEvent): void;
    onPointerMove(e: PointerEvent): void;
    onPointerUp(e: PointerEvent): void;
    onPointerCancel(e: PointerEvent): void;
    onLostPointerCapture(e: PointerEvent): void;
  };
  dragging: boolean;
  onMove: (delta: number) => void;
  onResize: (size: WidgetSize) => void;
  onRemove: () => void;
}

/** What the tile's menu adds to the widget menu, in every mode. */
export interface TileDashboardActions {
  dashboard: Dashboard;
  onEdit?: () => void;
}

function Item({ label, onClick }: { label: string; onClick: (e: MouseEvent) => void }) {
  return (
    <button type="button" class="thread-overflow-item widget-menu-item" role="menuitem" onClick={onClick}>
      <span class="widget-menu-label"><span>{label}</span></span>
    </button>
  );
}

function tileMenuItems(ctx: OverflowMenuContext, target: DashboardTileTarget, onEdit?: () => void): ComponentChildren {
  return (
    <>
      {dashboardTileMenuItems(ctx, target)}
      {onEdit && (
        <>
          <div class="thread-overflow-divider" role="separator" />
          <Item label="Edit dashboard" onClick={ctx.run(onEdit)} />
        </>
      )}
    </>
  );
}

/** The S · M · L pill. Only the sizes the widget declares are offered. */
function SizePill({ tile, onResize }: { tile: WidgetDashboardTile; onResize: (size: WidgetSize) => void }) {
  const offered = WIDGET_SIZES.filter((s) => tile.sizes.includes(s));
  if (offered.length < 2) return null;
  return (
    <div class="dashboard-size-pill" role="radiogroup" aria-label={`Size of ${tile.label ?? tile.name}`}>
      {offered.map((size) => (
        <button
          key={size}
          type="button"
          role="radio"
          aria-checked={tile.size === size}
          aria-label={size}
          class={tile.size === size ? 'is-current' : undefined}
          onClick={() => onResize(size)}
        >
          {SIZE_LETTERS[size]}
        </button>
      ))}
    </div>
  );
}

interface TileProps<T extends Tile> {
  tile: T;
  order: number;
  dashboardHidden: boolean;
  actions: TileDashboardActions;
  editing: TileEditing | null;
  /** A widget tile reports where its frame stands, under the tile's key. */
  onFrameState: (key: string, state: FrameState) => void;
}

/** One tile: a widget filling a box its size class sets, or an app shortcut. */
export function DashboardTile(props: TileProps<Tile>) {
  const { tile } = props;
  return tile.kind === 'app'
    ? <AppShortcutTile {...props} tile={tile} />
    : <WidgetTile {...props} tile={tile} />;
}

/** The edit-mode controls both kinds of tile share: the shield a drag grabs,
 *  and the remove badge. */
function EditControls({ name, editing, children }: { name: string; editing: TileEditing; children?: ComponentChildren }) {
  return (
    <>
      <div
        class="dashboard-tile-shield"
        role="button"
        tabIndex={0}
        aria-label={`Move ${name}. Drag it, or press Alt and an arrow key.`}
        {...editing.shield}
        onKeyDown={(e) => {
          if (!e.altKey) return;
          const delta = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 }[e.key];
          if (!delta) return;
          e.preventDefault();
          editing.onMove(delta);
        }}
      />
      <button
        type="button"
        class="dashboard-tile-remove"
        aria-label={`Remove ${name} from the dashboard`}
        onClick={editing.onRemove}
      >
        −
      </button>
      {children}
    </>
  );
}

/** The keyboard's menu key, or Shift+F10 where the keyboard has none. */
function isMenuKey(e: KeyboardEvent): boolean {
  return e.key === 'ContextMenu' || (e.shiftKey && e.key === 'F10');
}

/** An *app shortcut*: the app's icon over its name, drawn on the dashboard
 *  with no frame. A tap opens the app in the Canvas. A right-click, a finger
 *  held on it or the menu key opens its menu. No frame mounts, so a
 *  shortcut costs no renderer process (ADR 0227). */
function AppShortcutTile({ tile, order, actions, editing }: TileProps<AppDashboardTile>) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const key = dashboardTileKey(tile);
  const name = tile.label ?? tile.name;
  const register = editing?.register;
  const menu = useObjectMenu({ onTap: () => void openAppById(tile.app_id) });

  useEffect(() => {
    if (!register || !box) return undefined;
    register(key, box);
    return () => register(key, null);
  }, [register, box, key]);

  const classes = ['dashboard-tile', 'is-app-shortcut', editing && 'is-editing', editing?.dragging && 'is-dragging']
    .filter(Boolean).join(' ');
  return (
    <div
      ref={setBox}
      class={classes}
      data-dashboard-tile={key}
      data-kind="app"
      data-size={tile.size}
      style={gridPlace(tile.size, order)}
      {...(editing ? {} : menu.press)}
      onClick={editing ? undefined : menu.onClick}
      onKeyDown={editing ? undefined : (e) => {
        if (!box || !isMenuKey(e)) return;
        e.preventDefault();
        menu.open(box);
      }}
    >
      <button type="button" class="app-shortcut" aria-label={`Open ${name}`} tabIndex={editing ? -1 : undefined}>
        <AppIcon appId={tile.app_id} name={tile.name} icon={tile.icon} class="app-shortcut-icon" />
        <span class="app-shortcut-name">{name}</span>
      </button>
      {editing && <EditControls name={name} editing={editing} />}
      <OverflowMenu
        ariaLabel={`${name} shortcut actions`}
        stopPropagation
        hostOpener={{ ref: menu.opener, trigger: false }}
        items={(ctx) => (
          <>
            <Item label="Open" onClick={ctx.run(() => void openAppById(tile.app_id))} />
            <div class="thread-overflow-divider" role="separator" />
            <Item
              label="Remove from dashboard"
              onClick={ctx.run(() => void removeFromDashboard(actions.dashboard, { appId: tile.app_id }, name, 'app'))}
            />
            {actions.onEdit && <Item label="Edit dashboard" onClick={ctx.run(actions.onEdit)} />}
          </>
        )}
      />
    </div>
  );
}

/** A widget: it fills a box its size class sets. Content past the box
 *  is clipped under a fade, and the tile never scrolls inside. A right-click
 *  or a finger held on it opens the tile menu, which ends in Edit dashboard.
 *  In edit mode a shield covers the frame, so a drag moves the tile instead of
 *  reaching the widget, and a press opens no menu. */
function WidgetTile({ tile, order, dashboardHidden, actions, editing, onFrameState }: TileProps<WidgetDashboardTile>) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const [contentHeight, setContentHeight] = useState(0);
  const [clipped, setClipped] = useState(false);
  const key = dashboardTileKey(tile);
  const name = tile.label ?? tile.name;
  const register = editing?.register;
  const menu = useObjectMenu();

  useEffect(() => {
    if (!register || !box) return undefined;
    register(key, box);
    return () => register(key, null);
  }, [register, box, key]);

  useLayoutEffect(() => {
    if (!box) return undefined;
    // One pixel of slack absorbs `clientHeight` rounding a fractional report.
    const measure = () => setClipped(contentHeight - box.clientHeight > 1);
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    return () => observer.disconnect();
  }, [box, contentHeight]);

  const onEdit = actions.onEdit;
  const target: DashboardTileTarget = {
    dashboard: actions.dashboard,
    appId: tile.app_id,
    params: tile.params,
    label: tile.label,
    name,
  };
  const classes = [
    'dashboard-tile',
    tile.transparent && 'is-transparent',
    editing && 'is-editing',
    editing?.dragging && 'is-dragging',
  ].filter(Boolean).join(' ');
  return (
    <div
      ref={setBox}
      class={classes}
      data-dashboard-tile={key}
      data-kind="widget"
      data-size={tile.size}
      style={gridPlace(tile.size, order)}
      {...(editing ? {} : menu.press)}
      onClickCapture={editing ? undefined : menu.onClick}
    >
      <WidgetFrame
        appId={tile.app_id}
        params={tile.params}
        reveal={tile.reveal}
        place="dashboard"
        size={tile.size}
        visible={!dashboardHidden}
        onSize={(size) => setContentHeight(size.height)}
        onFrameState={(state) => onFrameState(key, state)}
        onMenu={(at) => { if (box && !editing) menu.open(box, at); }}
      />
      {clipped && !editing && <div class="dashboard-tile-fade" />}
      {editing && (
        <EditControls name={name} editing={editing}>
          <SizePill tile={tile} onResize={editing.onResize} />
        </EditControls>
      )}
      <OverflowMenu
        ariaLabel={`${name} widget actions`}
        hostOpener={{ ref: menu.opener, trigger: false }}
        items={(ctx) => tileMenuItems(ctx, target, onEdit)}
      />
    </div>
  );
}

/** The skeleton drawn from the tile's own box, at a typical first dashboard. */
export function TileSkeleton({ size }: { size: TileSize }) {
  return (
    <div class="dashboard-tile" data-size={size} style={gridPlace(size)}>
      <SkBlock w="100%" h="100%" />
    </div>
  );
}
