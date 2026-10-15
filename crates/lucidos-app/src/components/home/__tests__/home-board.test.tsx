// @vitest-environment jsdom
/**
 * The Home board (ADR 0432). Its fixed square cells take their column count
 * from the board's own width, and there are three size spans. Tiles take board
 * order through CSS `order` over a DOM order that never rearranges. Hidden
 * tiles mount no frame.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const frameProps = new Map<string, { place: string; size?: string; visible?: boolean }>();
vi.mock('../../widgets/WidgetFrame', () => ({
  WidgetFrame: (props: { appId: string; place: string; size?: string; visible?: boolean }) => {
    frameProps.set(props.appId, props);
    return <iframe data-frame={props.appId} />;
  },
}));

const api = vi.hoisted(() => ({
  arranged: [] as Array<{ layout: string; tiles: unknown[] }>,
  unpinned: [] as unknown[],
}));
vi.mock('../../../api/client/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/widgets')>()),
  arrangeHomeBoardApi: (layout: string, tiles: unknown[]) => {
    api.arranged.push({ layout, tiles });
    return new Promise(() => {});
  },
  unpinWidgetApi: (instance: unknown) => {
    api.unpinned.push(instance);
    return Promise.resolve({});
  },
}));

import type { HomeBoard as HomeBoardData, HomeBoardTile } from '../../../api/client/widgets';
import { homeBoards } from '../../../store/homeBoard';
import { threadMap } from '../../../store/store';
import { makeThreadState } from '../../../store/actions/threads-test-helpers';
import { HomeBoard, stableTileOrder } from '../HomeBoard';
import { BOARD_CELL_REM, BOARD_GAP_REM, BOARD_MIN_COLUMNS, BOARD_PAD_X_REM, boardColumns, boardContentWidth } from '../../../utils/boardGeometry';
import { SIZE_SPANS } from '../HomeBoardTile';

const HOME = 'home-thread';

function tile(appId: string, size: HomeBoardTile['size'], extra: Partial<HomeBoardTile> = {}): HomeBoardTile {
  return {
    app_id: appId,
    name: appId,
    reusable: true,
    reveal: 'on-load',
    sizes: ['small', 'medium', 'large'],
    pinned: true,
    size,
    ...extra,
  };
}

function setBoard(tiles: HomeBoardTile[]): void {
  const data: HomeBoardData = { thread_id: HOME, layout: 'desktop', tiles };
  homeBoards.value = new Map([['desktop', { status: 'loaded', data }]]);
}

describe('board geometry', () => {
  it('fits as many cells as the width holds, never fewer than two', () => {
    const rem = 16;
    const cell = BOARD_CELL_REM * rem;
    const gap = BOARD_GAP_REM * rem;
    expect(boardColumns(4 * cell + 3 * gap, rem)).toBe(4);
    expect(boardColumns(4 * cell + 3 * gap - 1, rem)).toBe(3);
    expect(boardColumns(100, rem)).toBe(BOARD_MIN_COLUMNS);
    expect(boardColumns(0, rem)).toBe(BOARD_MIN_COLUMNS);
  });

  it('counts columns inside the board\'s side padding', () => {
    const rem = 16;
    const fourCells = 4 * BOARD_CELL_REM * rem + 3 * BOARD_GAP_REM * rem;
    expect(boardContentWidth(fourCells + 2 * BOARD_PAD_X_REM * rem, rem)).toBe(fourCells);
    expect(boardColumns(boardContentWidth(fourCells, rem), rem)).toBe(3);
  });

  it('scales with the UI scale, since cells are in rem', () => {
    expect(boardColumns(1000, 16)).toBeGreaterThan(boardColumns(1000, 20));
  });

  it('spans one cell, two wide, and two by two', () => {
    expect(SIZE_SPANS).toEqual({
      small: { columns: 1, rows: 1 },
      medium: { columns: 2, rows: 1 },
      large: { columns: 2, rows: 2 },
    });
  });
});

describe('stableTileOrder', () => {
  it('appends new tiles and drops removed ones without reordering the rest', () => {
    expect(stableTileOrder([], ['a', 'b', 'c'])).toEqual(['a', 'b', 'c']);
    expect(stableTileOrder(['a', 'b', 'c'], ['c', 'a', 'b'])).toEqual(['a', 'b', 'c']);
    expect(stableTileOrder(['a', 'b', 'c'], ['c', 'd', 'a'])).toEqual(['a', 'c', 'd']);
  });
});

describe('<HomeBoard>', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    host = document.createElement('div');
    document.body.appendChild(host);
    const home = makeThreadState(HOME);
    home.meta.home = true;
    threadMap.value = new Map([[HOME, home]]);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    homeBoards.value = new Map();
  });

  const tiles = () => [...host.querySelectorAll<HTMLElement>('[data-home-tile]')];
  /** The tile's widget, read off the mocked frame inside it. */
  const app = (t: HTMLElement) => t.querySelector('iframe')?.dataset.frame;

  it('draws each tile at its size, with its board place as CSS order', () => {
    setBoard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    const drawn = tiles().map((t) => [app(t), t.dataset.size, t.style.order, t.style.gridColumn, t.style.gridRow]);
    expect(drawn).toEqual([
      ['clock', 'small', '0', 'span 1', 'span 1'],
      ['inbox', 'medium', '1', 'span 2', 'span 1'],
      ['prs', 'large', '2', 'span 2', 'span 2'],
    ]);
    expect(frameProps.get('inbox')).toMatchObject({ place: 'home', size: 'medium', visible: true });
  });

  /** Invariant: reordering never moves an iframe in the DOM. */
  it('reorders by CSS order and keeps every frame element where it was', () => {
    setBoard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    const before = [...host.querySelectorAll('iframe')];

    act(() => setBoard([tile('prs', 'large'), tile('clock', 'small'), tile('inbox', 'medium')]));
    const after = [...host.querySelectorAll('iframe')];
    expect(after).toEqual(before);
    expect(after.every((frame, i) => frame === before[i])).toBe(true);
    expect(tiles().map((t) => [app(t), t.style.order])).toEqual([
      ['clock', '1'],
      ['inbox', '2'],
      ['prs', '0'],
    ]);
  });

  /** Invariant: the cell edge is one constant, read by the grid and the head. */
  it('publishes its fixed cell on the board, for the grid and the head', () => {
    setBoard([tile('clock', 'small')]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    const board = host.querySelector<HTMLElement>('.home-board')!;
    expect(board.style.getPropertyValue('--home-board-cell')).toBe(`${BOARD_CELL_REM}rem`);
    expect(board.style.getPropertyValue('--home-board-gap')).toBe(`${BOARD_GAP_REM}rem`);
    expect(board.style.getPropertyValue('--home-board-columns')).toBe(String(BOARD_MIN_COLUMNS));
  });

  it('a tile hidden on this layout mounts no frame', () => {
    setBoard([tile('clock', 'small'), tile('inbox', 'medium', { hidden: true })]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    expect(tiles().map(app)).toEqual(['clock']);
    expect(frameProps.has('inbox')).toBe(false);
  });

  it('a hidden board tells its tiles they are not visible', () => {
    setBoard([tile('clock', 'small')]);
    act(() => render(<HomeBoard layout="desktop" hidden />, host));
    expect(frameProps.get('clock')?.visible).toBe(false);
  });

  it('a transparent widget draws a see-through tile', () => {
    setBoard([tile('clock', 'small', { transparent: true }), tile('inbox', 'medium')]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    expect(tiles().map((t) => t.classList.contains('is-transparent'))).toEqual([true, false]);
  });

  /** Invariant: the desktop rail's head carries no title; the phone's does (ADR 0468). */
  it('titles the head on the phone only', () => {
    setBoard([tile('clock', 'small')]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    expect(host.querySelector('.home-board-title')).toBeNull();
    expect(host.querySelector('.home-board-head .home-board-edit')).not.toBeNull();
    act(() => render(<HomeBoard layout="phone" />, host));
    expect(host.querySelector('.home-board-title')?.textContent).toBe('Home board');
  });

  it('an empty board says so and offers Add', () => {
    setBoard([]);
    act(() => render(<HomeBoard layout="desktop" />, host));
    expect(host.querySelector('.home-board-empty')?.textContent).toContain('Nothing is pinned to Home yet');
    expect(host.querySelector('.home-board-empty .home-board-add')).not.toBeNull();
  });
});

describe('<HomeBoard> in edit mode', () => {
  let host: HTMLDivElement;

  beforeEach(() => {
    frameProps.clear();
    api.arranged.length = 0;
    api.unpinned.length = 0;
    host = document.createElement('div');
    document.body.appendChild(host);
    const home = makeThreadState(HOME);
    home.meta.home = true;
    threadMap.value = new Map([[HOME, home]]);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
    homeBoards.value = new Map();
  });

  function edit() {
    act(() => render(<HomeBoard layout="desktop" />, host));
    act(() => host.querySelector<HTMLButtonElement>('.home-board-edit')!.click());
  }
  const tileOf = (appId: string) => host.querySelector<HTMLElement>(`[data-home-tile^="${appId}?"]`)!;

  it('shields each frame and offers only the sizes a widget declares', () => {
    setBoard([tile('clock', 'small', { sizes: ['small', 'large'] }), tile('inbox', 'medium', { sizes: ['medium'] })]);
    edit();
    expect(host.querySelectorAll('.home-board-tile-shield')).toHaveLength(2);
    const pill = [...tileOf('clock').querySelectorAll('.home-board-size-pill button')].map((b) => b.textContent);
    expect(pill).toEqual(['S', 'L']);
    expect(tileOf('inbox').querySelector('.home-board-size-pill')).toBeNull();
  });

  /** Invariant: a widget that declares no sizes comes with all three, so it
   *  offers the whole pill. */
  it('offers S, M and L on a widget that takes every size', () => {
    setBoard([tile('inbox', 'medium')]);
    edit();
    const pill = [...tileOf('inbox').querySelectorAll('.home-board-size-pill button')].map((b) => b.textContent);
    expect(pill).toEqual(['S', 'M', 'L']);
  });

  it('a size change writes one arrangement with every tile', () => {
    setBoard([tile('clock', 'small', { sizes: ['small', 'large'] }), tile('inbox', 'medium')]);
    edit();
    act(() => tileOf('clock').querySelector<HTMLButtonElement>('.home-board-size-pill button[aria-label="large"]')!.click());
    expect(api.arranged).toEqual([{
      layout: 'desktop',
      tiles: [{ app_id: 'clock', size: 'large' }, { app_id: 'inbox', size: 'medium' }],
    }]);
    expect(tileOf('clock').dataset.size).toBe('large');
  });

  it('Alt and an arrow moves a tile one place and writes one arrangement', () => {
    setBoard([tile('clock', 'small'), tile('inbox', 'medium'), tile('prs', 'large')]);
    edit();
    const frames = [...host.querySelectorAll('iframe')];
    const shield = tileOf('clock').querySelector<HTMLElement>('.home-board-tile-shield')!;
    act(() => { shield.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', altKey: true, bubbles: true })); });
    expect(api.arranged).toHaveLength(1);
    expect((api.arranged[0].tiles as Array<{ app_id: string }>).map((t) => t.app_id)).toEqual(['inbox', 'clock', 'prs']);
    expect([...host.querySelectorAll('iframe')]).toEqual(frames);

    act(() => { shield.dispatchEvent(new KeyboardEvent('keydown', { key: 'ArrowRight', bubbles: true })); });
    expect(api.arranged).toHaveLength(1);
  });

  it('shows a hidden tile as a place to bring back, with no frame', () => {
    setBoard([tile('clock', 'small'), tile('inbox', 'medium', { hidden: true })]);
    edit();
    expect(tileOf('inbox').classList.contains('is-hidden-tile')).toBe(true);
    expect(tileOf('inbox').querySelector('iframe')).toBeNull();
    act(() => tileOf('inbox').querySelector<HTMLButtonElement>('button')!.click());
    expect(api.arranged[0].tiles).toEqual([{ app_id: 'clock', size: 'small' }, { app_id: 'inbox', size: 'medium' }]);
  });

  it('the minus badge unpins the tile from Home', () => {
    setBoard([tile('clock', 'small')]);
    edit();
    act(() => tileOf('clock').querySelector<HTMLButtonElement>('.home-board-tile-remove')!.click());
    expect(api.unpinned).toEqual([expect.objectContaining({ appId: 'clock', threadId: HOME })]);
  });

  it('Done leaves edit mode', () => {
    setBoard([tile('clock', 'small')]);
    edit();
    act(() => host.querySelector<HTMLButtonElement>('.home-board-done')!.click());
    expect(host.querySelector('.home-board-tile-shield')).toBeNull();
    expect(host.querySelector('.home-board-edit')).not.toBeNull();
  });
});
