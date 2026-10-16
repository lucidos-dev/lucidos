/** Home board state (ADR 0432): the board of each layout class, as the engine
 *  folds it from Home's pins and its `HomeBoardArranged` snapshots.
 *
 *  The client keeps the last answer per layout class. A pin, an unpin or an
 *  arrangement on the home thread arriving over SSE re-reads it. */

import { signal } from '@preact/signals';
import type { HomeBoard } from '../api/client/widgets';
import type { LayoutClass } from '../generated/thread-event-wire';
import type { Loadable } from './types';

export const homeBoards = signal<ReadonlyMap<LayoutClass, Loadable<HomeBoard>>>(new Map());

export function homeBoardFor(layout: LayoutClass): Loadable<HomeBoard> {
  return homeBoards.value.get(layout) ?? { status: 'not-loaded' };
}

/** Which face of Home a phone shows in its middle pane: the board, or the
 *  Home chat slid up over it. A desktop shows both at once. */
export type HomeFace = 'board' | 'chat';

export const homeFace = signal<HomeFace>('chat');

let boardLanding = false;

/** The next arrival in Home lands on its board face. The Home icon, the
 *  Lucidos menu's Home row and a cold start with no stored focus ask for it.
 *  Every other way in, such as a link or a notification about a reply, lands
 *  on the chat face, where that reply is. */
export function landNextHomeOnBoard(): void {
  boardLanding = true;
}

/** The face an arrival in Home lands on, spending the request above. */
export function takeHomeLanding(): HomeFace {
  const face: HomeFace = boardLanding ? 'board' : 'chat';
  boardLanding = false;
  return face;
}
