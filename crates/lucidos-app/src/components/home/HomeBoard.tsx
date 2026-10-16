import type { ComponentChildren } from 'preact';
import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { HomeBoardTile as HomeBoardTileData } from '../../api/client/widgets';
import type { LayoutClass } from '../../generated/thread-event-wire';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { arrangeHomeBoard, rereadHomeBoard } from '../../store/actions/homeBoard';
import { homeThreadId } from '../../store/actions/homeThread';
import { unpinWidgetFromShelf } from '../../store/actions/widget-actions';
import { loadReusableWidgets } from '../../store/actions/widgets';
import { homeBoardFor } from '../../store/homeBoard';
import type { WidgetSize } from '../../store/types';
import { threadWidgetKey } from '../../store/widgets';
import { getRemPx } from '../../utils/dom';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { OverflowMenu } from '../shared/OverflowMenu';
import { WidgetPickerItems } from '../widgets/WidgetPickerItems';
import { SkeletonProvider } from '../shared/Skeleton';
import { useEscapeStep } from '../../hooks/useEscapeStep';
import { BOARD_CELL_REM, BOARD_GAP_REM, BOARD_MIN_COLUMNS, boardColumns, boardContentWidth } from '../../utils/boardGeometry';
import { inOrder, withTile } from './boardArrangement';
import { HiddenTile, HomeBoardTile, TileSkeleton } from './HomeBoardTile';
import { useBoardDrag } from './useBoardDrag';

/** The tiles' DOM order: the order each first appeared in, appended to and
 *  never rearranged. A tile's board place is its CSS `order`, because moving
 *  an iframe in the DOM reloads it (ADR 0419). */
export function stableTileOrder(previous: readonly string[], keys: readonly string[]): string[] {
  const present = new Set(keys);
  const kept = previous.filter((k) => present.has(k));
  const known = new Set(kept);
  return [...kept, ...keys.filter((k) => !known.has(k))];
}

const SKELETON_SIZES: WidgetSize[] = ['small', 'small', 'medium', 'medium'];

/** Add: the installed reusable widgets, the picker the thread menu's Add
 *  widget shows too. */
function AddWidget({ home }: { home: string }) {
  return (
    <OverflowMenu
      ariaLabel="Add a widget to Home"
      face={{ class: 'action-btn action-btn-secondary home-board-add', children: '+ Add' }}
      onOpen={() => void loadReusableWidgets()}
      items={({ run }) => <WidgetPickerItems run={run} threadId={home} />}
    />
  );
}

/** The Home board (ADR 0432): the widgets pinned to Home, in a grid of square
 *  cells. The desktop rail hosts it, and the phone's Home pane.
 *
 *  `hidden` is a board kept mounted out of sight, as in a hidden rail. Its
 *  tiles hear they are hidden and unload after the off-screen hold, so
 *  showing it again soon finds them as they were.
 *
 *  Edit mode lays a shield over each frame. A drag or Alt plus an arrow moves
 *  a tile. The pill resizes it, the minus unpins it from Home, and its menu
 *  hides it on this layout. Each drop or change writes one arrangement. Done
 *  or Escape leaves. */
export function HomeBoard({ layout, hidden = false, welcome = null }: {
  layout: LayoutClass;
  hidden?: boolean;
  /** Drawn above an empty board's Add: the phone's empty Home shows its
   *  welcome on the board face (ADR 0411, ADR 0432). */
  welcome?: ComponentChildren;
}) {
  const board = homeBoardFor(layout);
  const home = homeThreadId.value;
  const showLoading = useDelayedLoading(board);
  const [root, setRoot] = useState<HTMLDivElement | null>(null);
  const [columns, setColumns] = useState(BOARD_MIN_COLUMNS);
  const [editing, setEditing] = useState(false);
  const domOrder = useRef<string[]>([]);
  const all = board.status === 'loaded' ? board.data.tiles : [];
  const keys = all.map(threadWidgetKey);

  const commit = (tiles: HomeBoardTileData[]) => void arrangeHomeBoard(layout, tiles);
  const drag = useBoardDrag({ order: keys, enabled: editing, onDrop: (order) => commit(inOrder(all, order)) });
  useEscapeStep(editing ? () => setEditing(false) : null);
  // A hidden board cannot be edited, and its Escape step must not outlive it.
  useEffect(() => {
    if (hidden) setEditing(false);
  }, [hidden]);

  useEffect(() => {
    if (board.status === 'not-loaded') void rereadHomeBoard(layout);
  }, [board.status, layout]);

  // The board's own width, never the window's: on desktop the rail's divider
  // sets it.
  useLayoutEffect(() => {
    if (!root) return undefined;
    const measure = () => {
      const rem = getRemPx();
      setColumns(boardColumns(boardContentWidth(root.clientWidth, rem), rem));
    };
    measure();
    if (typeof ResizeObserver !== 'function') return undefined;
    const observer = new ResizeObserver(measure);
    observer.observe(root);
    return () => observer.disconnect();
  }, [root]);

  if (board.status === 'failed') return <LoadableError noun="the Home board" error={board.error} />;

  const drawn = all.filter((t) => editing || !t.hidden);
  const byKey = new Map(drawn.map((t) => [threadWidgetKey(t), t]));
  domOrder.current = stableTileOrder(domOrder.current, drawn.map(threadWidgetKey));
  const place = (key: string) => drag.shown.indexOf(key);
  // On the board, since its head lines up with the grid too.
  const geometry = {
    '--home-board-columns': String(columns),
    '--home-board-cell': `${BOARD_CELL_REM}rem`,
    '--home-board-gap': `${BOARD_GAP_REM}rem`,
  };
  const empty = board.status === 'loaded' && all.length === 0;
  const change = (key: string, patch: { size?: WidgetSize; hidden?: boolean }) => commit(withTile(all, key, patch));

  return (
    <div ref={setRoot} class={`home-board${editing ? ' is-editing' : ''}`} data-layout={layout} style={geometry}>
      <div class="home-board-head">
        {/* The desktop rail is titled by its own aria-label and needs no words (ADR 0468). */}
        {layout === 'phone' && <span class="home-board-title">Home board</span>}
        {home && !empty && <AddWidget home={home} />}
        {!empty && board.status === 'loaded' && (editing ? (
          <button type="button" class="action-btn home-board-done" onClick={() => setEditing(false)}>Done</button>
        ) : (
          <button type="button" class="action-btn action-btn-secondary home-board-edit" onClick={() => setEditing(true)}>Edit</button>
        ))}
      </div>
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={(
          <SkeletonProvider>
            <div class="home-board-grid">
              {SKELETON_SIZES.map((size, i) => <TileSkeleton key={i} size={size} />)}
            </div>
          </SkeletonProvider>
        )}
      >
        {empty ? (
          <div class="empty-state home-board-empty">
            {welcome}
            <p>Nothing is pinned to Home yet. Pin a widget and it shows here, live.</p>
            {home && <AddWidget home={home} />}
          </div>
        ) : (
          <div class="home-board-grid">
            {home && domOrder.current.map((key) => {
              const tile = byKey.get(key);
              if (!tile) return null;
              const name = tile.label ?? tile.name;
              if (tile.hidden) {
                return (
                  <HiddenTile
                    key={key}
                    tile={tile}
                    order={place(key)}
                    layout={layout}
                    register={drag.register}
                    onShow={() => change(key, { hidden: false })}
                  />
                );
              }
              return (
                <HomeBoardTile
                  key={key}
                  tile={tile}
                  order={place(key)}
                  boardHidden={hidden}
                  homeId={home}
                  board={{
                    layout,
                    onEdit: editing ? undefined : () => setEditing(true),
                    onHide: () => change(key, { hidden: true }),
                  }}
                  editing={editing ? {
                    layout,
                    register: drag.register,
                    shield: drag.shield(key),
                    dragging: drag.dragging === key,
                    onMove: (delta) => drag.moveBy(key, delta),
                    onResize: (size) => change(key, { size }),
                    onRemove: () => void unpinWidgetFromShelf({ appId: tile.app_id, threadId: home, params: tile.params }, name),
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
