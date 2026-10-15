import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useState } from 'preact/hooks';
import { WIDGET_SIZES } from '@lucidos/engine-constants';
import type { HomeBoardTile as Tile } from '../../api/client/widgets';
import type { LayoutClass } from '../../generated/thread-event-wire';
import { APP_FRAME_HOLD_EVENT, APP_FRAME_TAP_EVENT } from '../../store/actions/widget-frame-bridge';
import type { WidgetSize } from '../../store/types';
import { threadWidgetKey } from '../../store/widgets';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';
import { SkBlock } from '../shared/Skeleton';
import { WidgetFrame } from '../widgets/WidgetFrame';
import { widgetMenuItems, type WidgetMenuTarget } from '../widgets/WidgetMenu';

/** How long a tap on a touch screen keeps a tile's menu up. */
const CONTROLS_REVEAL_MS = 3000;

/** How many cells each widget size spans (ADR 0432). */
export const SIZE_SPANS: Record<WidgetSize, { columns: number; rows: number }> = {
  small: { columns: 1, rows: 1 },
  medium: { columns: 2, rows: 1 },
  large: { columns: 2, rows: 2 },
};

/** The size pill's letter for each size. */
const SIZE_LETTERS: Record<WidgetSize, string> = { small: 'S', medium: 'M', large: 'L' };

function gridPlace(size: WidgetSize, order?: number) {
  const span = SIZE_SPANS[size];
  return { order, gridColumn: `span ${span.columns}`, gridRow: `span ${span.rows}` };
}

/** What edit mode adds to a tile, and what it may do. */
export interface TileEditing {
  layout: LayoutClass;
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
export interface TileBoardActions {
  layout: LayoutClass;
  onEdit?: () => void;
  onHide: () => void;
}

function Item({ label, onClick }: { label: string; onClick: (e: MouseEvent) => void }) {
  return (
    <button type="button" class="thread-overflow-item widget-menu-item" role="menuitem" onClick={onClick}>
      <span class="widget-menu-label"><span>{label}</span></span>
    </button>
  );
}

function tileMenuItems(ctx: OverflowMenuContext, target: WidgetMenuTarget, board: TileBoardActions): ComponentChildren {
  return (
    <>
      {widgetMenuItems(ctx, target)}
      <div class="thread-overflow-divider" role="separator" />
      {board.onEdit && <Item label="Edit Home board" onClick={ctx.run(board.onEdit)} />}
      <Item label={`Hide on ${board.layout}`} onClick={ctx.run(board.onHide)} />
    </>
  );
}

/** The S · M · L pill. Only the sizes the widget declares are offered. */
function SizePill({ tile, onResize }: { tile: Tile; onResize: (size: WidgetSize) => void }) {
  const offered = WIDGET_SIZES.filter((s) => tile.sizes.includes(s));
  if (offered.length < 2) return null;
  return (
    <div class="home-board-size-pill" role="radiogroup" aria-label={`Size of ${tile.label ?? tile.name}`}>
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

/** One tile: the widget filling a box its size class sets. Content past the box
 *  is clipped under a fade, and the tile never scrolls inside. In edit mode a
 *  shield covers the frame, so a drag moves the tile instead of reaching the
 *  widget. */
export function HomeBoardTile({ tile, order, boardHidden, homeId, board, editing }: {
  tile: Tile;
  order: number;
  boardHidden: boolean;
  homeId: string;
  board: TileBoardActions;
  editing: TileEditing | null;
}) {
  const [box, setBox] = useState<HTMLDivElement | null>(null);
  const [contentHeight, setContentHeight] = useState(0);
  const [clipped, setClipped] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const key = threadWidgetKey(tile);
  const name = tile.label ?? tile.name;
  const register = editing?.register;

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

  // A press inside the frame reaches only the frame, so the SDK passes a tap
  // up, and the tile shows its menu for a while. A scroll is no tap.
  useEffect(() => {
    if (!box) return undefined;
    let hide: ReturnType<typeof setTimeout> | undefined;
    const onTap = (e: Event) => {
      if ((e as CustomEvent<string>).detail === 'mouse') return;
      setRevealed(true);
      clearTimeout(hide);
      hide = setTimeout(() => setRevealed(false), CONTROLS_REVEAL_MS);
    };
    box.addEventListener(APP_FRAME_TAP_EVENT, onTap);
    return () => {
      clearTimeout(hide);
      box.removeEventListener(APP_FRAME_TAP_EVENT, onTap);
    };
  }, [box]);

  // A finger held still on the tile enters edit mode. Touch drag moves a tile
  // only there: outside it, a drag over a frame stays the widget's.
  const onEdit = board.onEdit;
  useEffect(() => {
    if (!box || !onEdit) return undefined;
    box.addEventListener(APP_FRAME_HOLD_EVENT, onEdit);
    return () => box.removeEventListener(APP_FRAME_HOLD_EVENT, onEdit);
  }, [box, onEdit]);

  const target: WidgetMenuTarget = {
    threadId: homeId,
    appId: tile.app_id,
    params: tile.params,
    label: tile.label,
    name,
    reusable: tile.reusable,
    originPluginId: tile.origin_plugin_id,
    builtIn: tile.built_in ?? false,
    pinned: tile.pinned,
    shownEventId: tile.shown_event_id,
  };
  const classes = [
    'home-board-tile',
    tile.transparent && 'is-transparent',
    revealed && 'is-revealed',
    editing && 'is-editing',
    editing?.dragging && 'is-dragging',
  ].filter(Boolean).join(' ');
  return (
    <div
      ref={setBox}
      class={classes}
      data-home-tile={key}
      data-size={tile.size}
      style={gridPlace(tile.size, order)}
      onContextMenu={board.onEdit && !editing ? (e) => { e.preventDefault(); board.onEdit?.(); } : undefined}
    >
      <WidgetFrame
        appId={tile.app_id}
        params={tile.params}
        reveal={tile.reveal}
        place="home"
        size={tile.size}
        visible={!boardHidden}
        onSize={(size) => setContentHeight(size.height)}
      />
      {clipped && !editing && <div class="home-board-tile-fade" />}
      {editing && (
        <>
          <div
            class="home-board-tile-shield"
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
            class="home-board-tile-remove"
            aria-label={`Remove ${name} from Home`}
            onClick={editing.onRemove}
          >
            −
          </button>
          <SizePill tile={tile} onResize={editing.onResize} />
        </>
      )}
      <OverflowMenu
        ariaLabel={`${name} widget actions`}
        extraClass="home-board-tile-menu"
        items={(ctx) => tileMenuItems(ctx, target, board)}
      />
    </div>
  );
}

/** A tile this layout hides, as edit mode shows it: its place, and a way back. */
export function HiddenTile({ tile, order, layout, onShow, register }: {
  tile: Tile;
  order: number;
  layout: LayoutClass;
  onShow: () => void;
  register: (key: string, el: HTMLElement | null) => void;
}) {
  const key = threadWidgetKey(tile);
  return (
    <div
      ref={(el) => register(key, el)}
      class="home-board-tile is-hidden-tile"
      data-home-tile={key}
      data-size={tile.size}
      style={gridPlace(tile.size, order)}
    >
      <span class="home-board-hidden-label">{tile.label ?? tile.name}: hidden on {layout}</span>
      <button type="button" class="action-btn action-btn-secondary" onClick={onShow}>Show</button>
    </div>
  );
}

/** The skeleton drawn from the tile's own box, at a typical first board. */
export function TileSkeleton({ size }: { size: WidgetSize }) {
  return (
    <div class="home-board-tile" data-size={size} style={gridPlace(size)}>
      <SkBlock w="100%" h="100%" />
    </div>
  );
}
