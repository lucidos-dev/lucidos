import { useRef } from 'preact/hooks';
import type { ThreadHoldSummary } from '../../api/threads';
import type { ThreadState } from '../../store/thread-events';
import { focusedThreadId, setAsideChangeForThread, showToast, threadMap } from '../../store/store';
import { resumeHeldThread } from '../../store/actions/threadHold';
import { focusThread } from '../../store/actions/threads';
import { countWords, formatNotificationDate } from '../../utils/formatTime';
import { Disclosure } from '../shared/Disclosure';
import { HoldIcon } from '../shared/icons';

export function holdPlacer(hold: ThreadHoldSummary, threads: ReadonlyMap<string, ThreadState>): string {
  const by = hold.placed_by;
  if (by?.kind === 'device') return 'You';
  if (by?.kind === 'api' && by.source_thread_id) {
    const title = threads.get(by.source_thread_id)?.meta.title;
    return title ? `The agent of ${title}` : 'An agent';
  }
  return 'Lucidos';
}

export function holdWaiting(
  rootId: string,
  threads: ReadonlyMap<string, ThreadState>,
  hasSetAsideChange: (threadId: string) => boolean,
): string {
  let subThreads = 0;
  let changes = hasSetAsideChange(rootId) ? 1 : 0;
  for (const thread of threads.values()) {
    if (thread.meta.id === rootId || thread.meta.hold?.root_thread_id !== rootId) continue;
    subThreads += 1;
    if (hasSetAsideChange(thread.meta.id)) changes += 1;
  }
  const parts = [
    subThreads > 0 ? countWords(subThreads, 'sub-thread') : null,
    changes > 0 ? countWords(changes, 'change') : null,
  ].filter((part): part is string => part !== null);
  if (parts.length === 0) return 'Nothing else waits with it.';
  const verb = subThreads + changes === 1 ? 'waits' : 'wait';
  return `${parts.join(' and ')} ${verb} with it.`;
}

type Shown = {
  threadId: string;
  label: string;
  hint: string;
  detail: string;
  rootId: string | null;
  rootTitle: string;
};

function shownFor(thread: ThreadState, hold: ThreadHoldSummary): Shown {
  const threads = threadMap.value;
  const isRoot = hold.root_thread_id === thread.meta.id;
  const rootTitle = threads.get(hold.root_thread_id)?.meta.title || 'the thread it was placed on';
  const who = holdPlacer(hold, threads);
  const when = formatNotificationDate(new Date(hold.placed_at));
  const reach = isRoot
    ? holdWaiting(thread.meta.id, threads, (id) => !!setAsideChangeForThread(id))
    : `It came with the hold on ${rootTitle}, and Resume lives there.`;
  return {
    threadId: thread.meta.id,
    label: `On hold: ${hold.reason}`,
    hint: ` · ${who}, ${when}`,
    detail:
      `${who} put this thread on hold (${when}): “${hold.reason}”. Only messages wake it until someone resumes it. Finished sub-threads, background tasks, triggers and applied changes won't. ${reach} A message gets its answer, and the thread stays on hold.`,
    rootId: isRoot ? null : hold.root_thread_id,
    rootTitle,
  };
}

export function HoldStrip() {
  const threadId = focusedThreadId.value;
  const thread = threadId ? threadMap.value.get(threadId) : undefined;
  const hold = thread?.meta.hold ?? null;
  const shown = useRef<Shown | null>(null);
  if (thread && hold) shown.current = shownFor(thread, hold);
  const last = shown.current;
  return (
    <Disclosure open={!!hold}>
      {last && (
        <div class="hold-strip" data-role="hold-strip">
          <button
            class="hold-strip-reason"
            onClick={() => showToast(last.detail, 'info', { title: 'On hold' })}
          >
            <HoldIcon />
            <span class="hold-strip-text">
              <span class="hold-strip-label">{last.label}</span>
              <span class="hold-strip-hint">{last.hint}</span>
            </span>
          </button>
          {last.rootId === null ? (
            <button class="action-btn protected-surface" onClick={() => void resumeHeldThread(last.threadId)}>
              Resume
            </button>
          ) : (
            <button
              class="action-btn action-btn-secondary protected-surface"
              data-tooltip={`Resume lives on ${last.rootTitle}`}
              onClick={() => focusThread(last.rootId!)}
            >
              Open {last.rootTitle}
            </button>
          )}
        </div>
      )}
    </Disclosure>
  );
}
