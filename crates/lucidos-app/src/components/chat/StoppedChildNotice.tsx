import type { ThreadMeta } from '../../store/thread-events';
import { threadLink } from './ChildCompletionRow';
import { eventRowBody } from './EventRow';

export const STOPPED_CHILD_CONTINUE = 'Send a message to continue it';
export const STOPPED_CHILD_SETTLE = 'or Archive it, or Discard its change, to tell the parent you are done';

export function StoppedChildNotice({ meta }: { meta: ThreadMeta }) {
  if (!meta.isStoppedChild || !meta.parentThreadId) return null;
  return (
    <div class="stopped-child-notice" data-role="stopped-child-notice">
      {eventRowBody({
        kind: 'child',
        state: 'stopped',
        role: 'stopped-child-notice-row',
        subject: (
          <>
            {'You stopped this thread, and its parent is waiting on it: '}
            {threadLink(meta.parentThreadId, meta.parentThreadTitle)}
          </>
        ),
        stateLabel: 'Waiting for you',
        tone: 'live',
        facts: [
          { kind: 'text', text: STOPPED_CHILD_CONTINUE },
          { kind: 'text', text: STOPPED_CHILD_SETTLE },
        ],
      })}
    </div>
  );
}
