/** The size of a widget instance's cards in a thread. The engine holds it as
 *  `InlineWidgetSized` thread events, written through `POST
 *  /api/v1/widgets/size`, and the newest one wins. A pick on another device
 *  arrives as an event over SSE, so the card follows it.
 *
 *  A page can leave a pick behind it. The server's `cardSizes` stand in for
 *  those, and compete with the loaded events by sequence. */

import { threadMap } from './store';
import { getThreadEventsBump } from './threadActivity';
import type { WidgetSize } from './types';
import { widgetInstanceKey, type WidgetParams } from '../utils/widgetParams';

/** The newest size picked for this instance's cards, or `undefined`. A live
 *  pick changes no thread meta, so it bumps the thread's events signal and not
 *  `threadMap`. Reading the bump is what re-renders a card on a pick. */
export function widgetCardSize(threadId: string, appId: string, params?: WidgetParams): WidgetSize | undefined {
  getThreadEventsBump(threadId);
  const thread = threadMap.value.get(threadId);
  if (!thread) return undefined;
  const key = widgetInstanceKey(appId, params);
  let bestSeq = -Infinity;
  let best: WidgetSize | undefined;
  const consider = (seq: number, pickAppId: string, pickParams: unknown, size: WidgetSize) => {
    if (seq <= bestSeq) return;
    if (widgetInstanceKey(pickAppId, pickParams as WidgetParams | undefined) !== key) return;
    bestSeq = seq;
    best = size;
  };
  for (const pick of thread.cardSizes ?? []) consider(pick.sequence, pick.app_id, pick.params, pick.size);
  for (const [seq, event] of thread.events) {
    if (event.type === 'InlineWidgetSized') consider(seq, event.app_id, event.params, event.size);
  }
  return best;
}
