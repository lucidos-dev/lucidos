/** What the Home board reads, and how SSE keeps it fresh (ADR 0432). */

import { arrangeHomeBoardApi, fetchHomeBoard, type HomeBoardTile } from '../../api/client/widgets';
import type { ArrangedTile, LayoutClass } from '../../generated/thread-event-wire';
import { errorDetail } from '../../utils/errorDetail';
import { showToast } from '../store';
import { homeBoardFor, homeBoards } from '../homeBoard';
import { loadingIfFresh, toFailed } from '../types';

function setHomeBoard(layout: LayoutClass, value: ReturnType<typeof homeBoardFor>): void {
  const next = new Map(homeBoards.value);
  next.set(layout, value);
  homeBoards.value = next;
}

/** Per layout, the newest read started. Only its answer is applied, so an
 *  older read that lands late cannot put back a tile a newer one removed. */
const newestRead = new Map<LayoutClass, number>();
let lastTicket = 0;

/** Read the board of one layout class. A re-read keeps the tiles on screen. */
export function rereadHomeBoard(layout: LayoutClass): Promise<void> {
  const ticket = ++lastTicket;
  newestRead.set(layout, ticket);
  setHomeBoard(layout, loadingIfFresh(homeBoardFor(layout)));
  return fetchHomeBoard(layout).then(
    (data) => {
      if (newestRead.get(layout) === ticket) setHomeBoard(layout, { status: 'loaded', data });
    },
    (error) => {
      if (newestRead.get(layout) !== ticket) return;
      // A re-read that fails keeps what it had; only a first read fails.
      if (homeBoardFor(layout).status !== 'loaded') setHomeBoard(layout, toFailed(error));
    },
  );
}

/** What the arrange call carries: identity, size and hidden, nothing else. */
export function toArranged(tiles: readonly HomeBoardTile[]): ArrangedTile[] {
  return tiles.map((t) => ({
    app_id: t.app_id,
    ...(t.params ? { params: t.params } : {}),
    size: t.size,
    ...(t.hidden ? { hidden: true } : {}),
  }));
}

/** Arrange the board of one layout class: the tiles in their new order, with
 *  their sizes and hidden flags. The board shows it at once. One call writes
 *  one `HomeBoardArranged`, and a refusal puts the stored board back. */
export async function arrangeHomeBoard(layout: LayoutClass, tiles: HomeBoardTile[]): Promise<void> {
  const current = homeBoardFor(layout);
  if (current.status !== 'loaded') return;
  const ticket = ++lastTicket;
  newestRead.set(layout, ticket);
  setHomeBoard(layout, { status: 'loaded', data: { ...current.data, tiles } });
  try {
    const data = await arrangeHomeBoardApi(layout, toArranged(tiles));
    if (newestRead.get(layout) === ticket) setHomeBoard(layout, { status: 'loaded', data });
  } catch (error) {
    showToast(`Couldn't arrange the Home board: ${errorDetail(error)}`, 'error');
    void rereadHomeBoard(layout);
  }
}

/** Home's pins or arrangement changed, or the stream came back after a gap.
 *  Only a board this device has read is re-read. */
export function onHomeBoardEvent(): void {
  for (const [layout, board] of homeBoards.value) {
    if (board.status !== 'not-loaded') void rereadHomeBoard(layout);
  }
}

/** An app changed or went away: re-read each board that shows it. */
export function refreshHomeBoardsNaming(appId: string): void {
  for (const [layout, board] of homeBoards.value) {
    if (board.status === 'loaded' && board.data.tiles.some((t) => t.app_id === appId)) {
      void rereadHomeBoard(layout);
    }
  }
}

/** Test-only: forget every board and read. */
export function _resetHomeBoardsForTesting(): void {
  homeBoards.value = new Map();
  newestRead.clear();
}
