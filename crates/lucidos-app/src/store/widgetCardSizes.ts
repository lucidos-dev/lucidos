
import { threadMap } from './store';
import { getThreadEventsBump } from './threadActivity';
import type { WidgetSize } from './types';
import { widgetInstanceKey, type WidgetParams } from '../utils/widgetParams';

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
